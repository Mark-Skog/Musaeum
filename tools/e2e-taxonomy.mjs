#!/usr/bin/env node
// Phase 6, Part 1, in the real window, driven through the WebView2's debugging protocol with the
// input a person gives: the library's side (categories, tags, collections, smart collections,
// custom fields) and the editor beside the books.
//
//   node tools/e2e-taxonomy.mjs [--build target/release] [--out FILE.json] [--require-idle SECONDS]
//
// Four books written for this (tools/lib/test-books.mjs) go in a fresh library. Through the side:
// a tree of categories is made, a category renamed and moved, and the dialog that moves one offers
// no place under itself; a tag, a collection and a smart collection are made, and a custom field.
// Through the editor: a book's title, people and their roles, year, language, ISBN, rating,
// status, category, tags, collection and custom field are changed and saved, and every field set
// is the person's. A click on the chosen book keeps it chosen, Escape puts the details away, and a
// book the filter left behind is no longer chosen. Each filter of the side lists what it should, a category with what is under it,
// a collection in its order, which the editor changes. Then the book's file is replaced by a new edition with another title
// and author: what the person set stays. Closed and opened again, all of it is still there.
//
// The library and the books live in a temporary folder, removed at the end. The run waits for the
// machine to be free and stops as soon as someone uses it (exit code 3).

import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, idleSeconds, launch, press, typeInto, waitFor } from './lib/cdp.mjs';
import { epub, sentence } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'e2e-taxonomy', 'report.json'),
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
    console.error(`  FAILED: ${label} ${JSON.stringify(detail)}`);
  } else {
    console.log(`  ok: ${label}`);
  }
  return ok;
}

/** A book of two chapters, by `creators`. */
function book(title, creators, identifier, seed) {
  return epub({
    title,
    identifier,
    creators,
    chapters: [1, 2].map((chapter) => ({
      title: `Capítulo ${chapter}`,
      paragraphs: [0, 1, 2].map((paragraph) => sentence(`${seed}-${chapter}`, seed * 100 + chapter * 10 + paragraph, 14)),
    })),
  });
}

/** Marks the first element `selector` matches whose text is `text`, and clicks it. */
async function clickText(main, selector, text) {
  checkStillQuiet();
  const found = await waitFor(
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
  if (found) {
    await click(main, '[data-e2e-target="1"]');
  }
}

const taxonomy = (main) => main.evaluate('window.musaeumApp.taxonomy()');
const metadata = (main, work) => main.evaluate(`window.musaeumApp.workMetadata(${work})`);
const rows = (main) => main.evaluate('window.musaeumApp.rows()');

/** The titles listed once the list shows what `kind`/`id` asks for. */
async function listed(main, kind, id) {
  const shown = await waitFor(
    main,
    `(() => { const r = window.musaeumApp.rows(); const f = r.filter; return f && f.kind === ${JSON.stringify(kind)} && (f.kind === 'all' || f.id === ${JSON.stringify(id)}) ? r : null; })()`,
    { timeoutMs: 10_000, what: `the books of ${kind} ${id}` },
  );
  return shown.rows.map((row) => row.title);
}

/** Types a name in the open dialog and submits it. */
async function nameDialog(main, name) {
  await waitFor(main, "document.querySelector('.library-dialog input') !== null", { timeoutMs: 5000, what: 'the dialog' });
  await typeInto(main, '.library-dialog input', name);
  await press(main, 'Enter');
  await waitFor(main, "document.querySelector('.library-dialog') === null", { timeoutMs: 5000, what: 'the dialog to close' });
}

async function rowMenu(main, label, item) {
  await click(main, `[aria-label="Ações de ${label}"]`);
  await clickText(main, '[role="menuitem"]', item);
}

async function selectBook(main, title) {
  const list = await rows(main);
  const row = list.rows.find((candidate) => candidate.title === title);
  if (row === undefined) {
    throw new Error(`${title} is not listed`);
  }
  await click(main, `[data-document="${row.documentId}"]`);
  await waitFor(main, `document.querySelector('.inspector[data-work="${row.workId}"]') !== null`, { timeoutMs: 8000, what: `the details of ${title}` });
  return row;
}

async function save(main) {
  await waitFor(main, "(() => { const b = document.querySelector('#save-work'); return b && !b.disabled; })()", { timeoutMs: 5000, what: 'something to save' });
  await click(main, '#save-work');
  // The button is also off while the save runs: done is off with the panel no longer busy.
  await waitFor(main, "(() => { const b = document.querySelector('#save-work'); const a = document.querySelector('.inspector'); return b && b.disabled && a && a.getAttribute('aria-busy') !== 'true'; })()", { timeoutMs: 8000, what: 'the save' });
}

/** Chooses `text` in the combo box labelled `label` of the editor. */
async function addChip(main, label, typed, option) {
  const selector = `.inspector input[aria-label="${label}"], .inspector [aria-label="${label}"] input`;
  await typeInto(main, selector, typed);
  if (option === null) {
    await press(main, 'Enter');
    return null;
  }
  // What the list offers once it has the option, and whether a pick closes it: open, it covers the
  // fields below.
  const offered = await waitFor(main, `(() => { const o = [...document.querySelectorAll('.select-popover [role="option"]')].map((n) => n.textContent.trim()); return o.includes(${JSON.stringify(option)}) ? o : null; })()`, { timeoutMs: 5000, what: `the option ${option}` });
  await clickText(main, '.select-popover [role="option"]', option);
  const closed = await waitFor(main, "document.querySelector('.select-popover') === null", { timeoutMs: 3000, what: 'the list to close' }).catch(() => false);
  const focus = await main.evaluate("(() => { const a = document.activeElement; return a ? `${a.tagName} ${a.getAttribute('aria-label') ?? ''}` : null; })()");
  return { offered, closed: closed === true, focus };
}

async function run(work) {
  const shelf = join(work, 'estante');
  mkdirSync(shelf, { recursive: true });
  writeFileSync(join(shelf, 'vida.epub'), book('Vida Maravilhosa', ['Stephen Jay Gould'], 'urn:isbn:9788535902778', 1));
  writeFileSync(join(shelf, 'cosmos.epub'), book('Cosmos', ['Carl Sagan'], 'urn:x:cosmos', 2));
  writeFileSync(join(shelf, 'dom.epub'), book('Dom Casmurro', ['Machado de Assis'], 'urn:x:dom', 3));
  writeFileSync(join(shelf, 'gene.epub'), book('O Gene Egoísta', ['Richard Dawkins'], 'urn:x:gene', 4));
  const library = join(work, 'biblioteca');
  let session = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  let main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    await waitFor(main, '(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === 4 && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r : null; })()', {
      timeoutMs: 120_000,
      what: 'the four books',
    });

    // 1. Categories: a tree, a rename, a move, and no place under itself to move one to.
    await click(main, '[aria-label="Nova categoria"]');
    await nameDialog(main, 'Ciências');
    await rowMenu(main, 'Ciências', 'Nova subcategoria…');
    await nameDialog(main, 'Biologia');
    await rowMenu(main, 'Ciências', 'Nova subcategoria…');
    await nameDialog(main, 'Física');
    await click(main, '[aria-label="Nova categoria"]');
    await nameDialog(main, 'Literatura');
    await rowMenu(main, 'Física', 'Renomear…');
    await nameDialog(main, 'Física Moderna');
    await rowMenu(main, 'Física Moderna', 'Mover para…');
    await click(main, '.library-dialog .select-button');
    await clickText(main, '[role="option"]', 'Literatura');
    // Enter on the list's button opens the list again, as a select does; the dialog is sent with its button.
    await clickText(main, '.library-dialog button[type="submit"]', 'Mover');
    await waitFor(main, "document.querySelector('.library-dialog') === null", { timeoutMs: 5000, what: 'the move' });
    let tree = await taxonomy(main);
    const byName = (name) => tree.categories.find((category) => category.name === name);
    step('the categories are a tree, renamed and moved as asked', byName('Biologia')?.parentId === byName('Ciências')?.id && byName('Física Moderna')?.parentId === byName('Literatura')?.id && byName('Física') === undefined, {
      categories: tree.categories.map((category) => [category.name, category.parentId, category.depth]),
    });
    await rowMenu(main, 'Ciências', 'Mover para…');
    await click(main, '.library-dialog .select-button');
    const offered = await waitFor(main, "(() => { const o = [...document.querySelectorAll('[role=\"option\"]')].map((n) => n.textContent.trim()); return o.length > 0 ? o : null; })()", { timeoutMs: 5000, what: 'the places' });
    step('a category is never offered a place under itself', !offered.some((text) => text.startsWith('Ciências')), { offered });
    await press(main, 'Escape');
    await press(main, 'Escape');
    await waitFor(main, "document.querySelector('.library-dialog') === null", { timeoutMs: 5000, what: 'the dialog to close' });

    // 2. A collection and a custom field, through the side.
    await click(main, '[aria-label="Nova coleção"]');
    await nameDialog(main, 'Para as férias');
    await clickText(main, 'button', 'Campos personalizados…');
    await typeInto(main, '.field-add input', 'Situação');
    await click(main, '.field-add .select-button');
    await clickText(main, '[role="option"]', 'Lista de opções');
    await typeInto(main, '.field-add textarea', 'em casa\nemprestado');
    await clickText(main, 'button', 'Acrescentar campo');
    await waitFor(main, "document.querySelector('.field-list') !== null", { timeoutMs: 5000, what: 'the field' });
    await clickText(main, 'button', 'Fechar');
    tree = await taxonomy(main);
    step('a collection and a custom field are made', tree.collections.length === 1 && tree.fields.length === 1 && tree.fields[0].kind === 'choice', { collections: tree.collections, fields: tree.fields });

    // 3. The editor: every field of a book, saved, and each one the person's.
    const vida = await selectBook(main, 'Vida Maravilhosa');
    await typeInto(main, '.inspector input[data-field="title"]', 'Vida Maravilhosa (ed. revista)');
    await clickText(main, '.inspector button', 'Acrescentar pessoa');
    await typeInto(main, '.inspector input[aria-label="Nome 2"]', 'Paulo Geiger');
    await click(main, '.inspector [data-picker="Papel 2"] .select-button');
    await clickText(main, '[role="option"]', 'Tradutor');
    await typeInto(main, '.inspector input[data-field="year"]', '1990');
    await typeInto(main, '.inspector input[data-field="language"]', 'pt-br');
    await click(main, '.inspector [aria-label="4 estrelas"]');
    await click(main, '.inspector [data-picker="Estado de leitura"] .select-button');
    await clickText(main, '[role="option"]', 'Lendo');
    const picked = await addChip(main, 'Acrescentar categorias', 'Biolo', 'Ciências › Biologia');
    step('the picker lists only what has what was typed, and a pick closes it', JSON.stringify(picked.offered) === JSON.stringify(['Ciências › Biologia']) && picked.closed, picked);
    await addChip(main, 'Acrescentar etiquetas', 'Clássico', null);
    await addChip(main, 'Acrescentar coleções', 'férias', 'Para as férias');
    await click(main, '.inspector [data-picker="Situação"] .select-button');
    await clickText(main, '[role="option"]', 'emprestado');
    await save(main);
    let edited = await metadata(main, vida.workId);
    tree = await taxonomy(main);
    const biology = tree.categories.find((category) => category.name === 'Biologia');
    const user = ['title', 'creators', 'year', 'language', 'rating', 'status'].every((field) => edited.provenance[field] === 'user');
    step(
      'the editor saves every field, and each one becomes the person’s',
      edited.title === 'Vida Maravilhosa (ed. revista)' &&
        edited.creators.length === 2 &&
        edited.creators[1].name === 'Paulo Geiger' &&
        edited.creators[1].role === 'translator' &&
        edited.year === 1990 &&
        edited.language === 'pt-BR' &&
        edited.rating === 4 &&
        edited.status === 'reading' &&
        edited.categories.length === 1 &&
        edited.categories[0] === biology?.id &&
        edited.tags.length === 1 &&
        edited.collections.length === 1 &&
        edited.custom[0]?.value === 'emprestado' &&
        edited.identifiers[0]?.value === '9788535902778' &&
        user,
      { edited },
    );

    // Two more books in the collection, in the order they join it.
    let cosmos = null;
    for (const title of ['Dom Casmurro', 'Cosmos']) {
      cosmos = await selectBook(main, title);
      await addChip(main, 'Acrescentar coleções', 'férias', 'Para as férias');
      await save(main);
    }

    // A click on the chosen book keeps it chosen, as in a file manager; Escape puts the details away.
    await click(main, `[data-document="${cosmos.documentId}"]`);
    await delay(600);
    step('a click on the chosen book keeps it chosen', (await main.evaluate(`document.querySelector('.inspector[data-work="${cosmos.workId}"]') !== null`)) === true);
    await press(main, 'Escape');
    const putAway = await waitFor(main, "document.querySelector('.inspector') === null", { timeoutMs: 5000, what: 'the details put away' }).catch(() => false);
    step('Escape puts the details away', putAway === true);
    await selectBook(main, 'Cosmos');

    // 4. Each filter of the side.
    await clickText(main, '.folder-item .folder-name', 'Ciências');
    let titles = await listed(main, 'category', tree.categories.find((category) => category.name === 'Ciências').id);
    step('a category lists the books of those under it', JSON.stringify(titles) === JSON.stringify(['Vida Maravilhosa (ed. revista)']), { titles });
    tree = await taxonomy(main);
    await clickText(main, '.side-item .folder-name', 'Clássico');
    titles = await listed(main, 'tag', tree.tags[0].id);
    step('a tag lists its books', JSON.stringify(titles) === JSON.stringify(['Vida Maravilhosa (ed. revista)']), { titles });
    await clickText(main, '.side-item .folder-name', 'Para as férias');
    titles = await listed(main, 'collection', tree.collections[0].id);
    step('a collection lists its books in its order', JSON.stringify(titles) === JSON.stringify(['Vida Maravilhosa (ed. revista)', 'Dom Casmurro', 'Cosmos']), { titles });
    // Cosmos was chosen before the category, which does not list it, so it is no longer chosen here.
    step('a book left behind by the filter is not chosen on returning', (await main.evaluate("document.querySelector('.inspector') === null")) === true);
    // Moved up in the collection, from the editor.
    await selectBook(main, 'Cosmos');
    await clickText(main, '.inspector button', '↑ Subir');
    const moved = ['Vida Maravilhosa (ed. revista)', 'Cosmos', 'Dom Casmurro'];
    const reordered = await waitFor(main, `(() => { const t = window.musaeumApp.rows().rows.map((r) => r.title); return JSON.stringify(t) === ${JSON.stringify(JSON.stringify(moved))} ? t : null; })()`, { timeoutMs: 8000, what: 'the new order' }).catch(() => null);
    step('a book moves up in its collection, and only it', reordered !== null, { titles: reordered ?? (await rows(main)).rows.map((row) => row.title) });

    // 5. A smart collection, written in its dialog.
    await click(main, '[aria-label="Nova coleção inteligente"]');
    await typeInto(main, '.library-dialog input', 'Divulgação');
    await click(main, '.rule [data-picker="Campo"] .select-button');
    await clickText(main, '[role="option"]', 'Autor');
    await typeInto(main, '.rule input[aria-label="Valor"]', 'SAGAN');
    await waitFor(main, "document.querySelector('.rules-footer .muted')?.textContent.includes('1 livro')", { timeoutMs: 8000, what: 'the count' });
    await clickText(main, '.library-dialog button', 'Criar');
    await waitFor(main, "document.querySelector('.library-dialog') === null", { timeoutMs: 5000, what: 'the smart collection' });
    tree = await taxonomy(main);
    await clickText(main, '.side-item .folder-name', 'Divulgação');
    titles = await listed(main, 'smart', tree.smart[0].id);
    step('a smart collection finds what its rule says, without case', JSON.stringify(titles) === JSON.stringify(['Cosmos']) && tree.smart[0].works === 1, { titles, smart: tree.smart });
    await clickText(main, '.folder-item .folder-name', 'Todos os livros');
    await listed(main, 'all', null);

    // 6. A new edition of the file, with another title and author: what the person set stays.
    const target = join(shelf, 'vida.epub');
    writeFileSync(`${target}.tmp`, book('Wonderful Life', ['Outro Autor'], 'urn:isbn:9788535902778', 9));
    renameSync(`${target}.tmp`, target);
    await waitFor(
      main,
      `(() => { const r = window.musaeumApp.rows(); const b = r.rows.find((x) => x.workId === ${vida.workId}); return b && b.blake3 !== null && b.blake3 !== ${JSON.stringify(vida.blake3)} && !window.musaeumApp.library().indexing; })()`,
      { timeoutMs: 120_000, what: 'the new edition read' },
    );
    edited = await metadata(main, vida.workId);
    step('a new edition of the file does not write over what the person set', edited.title === 'Vida Maravilhosa (ed. revista)' && edited.creators[1]?.name === 'Paulo Geiger' && edited.year === 1990, { edited });
  } finally {
    report.exitCode = await closeApp(session).catch((error) => String(error));
    report.console = session.console.slice();
  }

  // 7. Closed and opened again.
  session = await launch(app, ['--library', library, '--e2e', '--width', '1400', '--height', '900'], 'window.musaeumApp !== undefined');
  main = session.cdp;
  try {
    await waitFor(main, "document.querySelector('.library') !== null", { what: 'the library' });
    const tree = await taxonomy(main);
    const list = await waitFor(main, '(() => { const r = window.musaeumApp.rows(); return r.total === 4 ? r : null; })()', { timeoutMs: 30_000, what: 'the books' });
    const vida = list.rows.find((row) => row.title === 'Vida Maravilhosa (ed. revista)');
    const kept = vida === undefined ? null : await metadata(main, vida.workId);
    step(
      'closed and opened again, everything is still there',
      tree.categories.length === 4 && tree.tags.length === 1 && tree.collections[0]?.works === 3 && tree.smart.length === 1 && tree.fields.length === 1 && kept?.rating === 4,
      { categories: tree.categories.length, kept },
    );
  } finally {
    report.reopenExitCode = await closeApp(session).catch((error) => String(error));
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
  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-taxonomy-'));
  try {
    await run(work);
  } finally {
    for (let attempt = 0; attempt < 20 && existsSync(work); attempt++) {
      try {
        rmSync(work, { recursive: true, force: true });
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
