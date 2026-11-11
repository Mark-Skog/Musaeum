#!/usr/bin/env node
// The flowing reader on its own, with no window: a headless Chrome, with a profile of its own, runs
// the app's reader page on normalized books from a cache, through the same messages the app's page
// sends, and checks the position gate of Phase 3 in each book.
//
//   node tools/reader-check.mjs --cache DIR [--dist apps/desktop/dist] [--chrome PATH] [--out FILE]
//
// --cache is a folder with normalized books in it, as `corpus-check --flow-only` leaves them
// (`flow/v1/<ab>/<hash>/musaeum.json` and the package beside it); every book found is checked.
// In each, the reader goes to the middle of the book and the check changes the font size, line
// height, margins, font, theme and screen size, and switches between paginated and continuous
// reading. After every change, the anchor has to be the same character, on the page shown
// (paginated) or within one line of the top of the screen (continuous). Then a page is turned, and
// where the normalizer cut a document, the parts have to meet as two paragraphs do. The reader
// page is served the way the app serves it: from an origin of its own, with the same policy.

import { spawn } from 'node:child_process';
import { readdirSync, readFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve, sep } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { Cdp, connectFrame, waitFor } from './lib/cdp.mjs';
import { SEAM_TOLERANCE_PX, seamOracle } from './lib/seam-oracle.mjs';

const root = resolve(import.meta.dirname, '..');
const arg = (name, fallback) => {
  const at = process.argv.indexOf(name);
  return at >= 0 ? process.argv[at + 1] : fallback;
};
const cache = resolve(arg('--cache', ''));
const dist = resolve(arg('--dist', join(root, 'apps', 'desktop', 'dist')));
const chrome = arg('--chrome', 'C:/Program Files/Google/Chrome/Application/chrome.exe');
const out = resolve(arg('--out', join(root, 'bench-results', 'reader-check', 'report.json')));

function findBooks(folder) {
  const books = [];
  const walk = (at) => {
    for (const name of readdirSync(at, { withFileTypes: true })) {
      const path = join(at, name.name);
      if (name.isDirectory()) {
        if (existsSync(join(path, 'musaeum.json'))) {
          books.push({ id: name.name, dir: path });
        } else {
          walk(path);
        }
      }
    }
  };
  walk(folder);
  return books;
}

// --only: just the books whose id starts with one of these prefixes, separated by + or commas.
const only = arg('--only', '').split(/[,+]/).filter((prefix) => prefix.length > 0);
const books = findBooks(cache).filter((book) => only.length === 0 || only.some((prefix) => book.id.startsWith(prefix)));
if (books.length === 0) {
  throw new Error(`no normalized books in ${cache}`);
}

const TYPES = {
  '.xhtml': 'application/xhtml+xml',
  '.html': 'application/xhtml+xml',
  '.opf': 'application/oebps-package+xml',
  '.xml': 'application/xml',
  '.ncx': 'application/xml',
  '.json': 'application/json',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.otf': 'font/otf',
  '.js': 'text/javascript',
};

let hostOrigin = '';
let bookOrigin = '';

/** The policy the app gives the reader's page, with the host's origin as the one that may frame it. */
function policy(script) {
  return [
    "default-src 'none'",
    script,
    "style-src 'unsafe-inline' blob:",
    'img-src blob: data:',
    'font-src blob: data:',
    'media-src blob:',
    "connect-src 'self'",
    'frame-src blob:',
    "worker-src 'none'",
    "object-src 'none'",
    "base-uri 'none'",
    "form-action 'none'",
    "manifest-src 'none'",
    `frame-ancestors ${hostOrigin}`,
  ].join('; ');
}

const bookServer = http.createServer((request, response) => {
  const url = new URL(request.url, bookOrigin);
  if (url.pathname === '/favicon.ico') {
    response.writeHead(204);
    return response.end();
  }
  const parts = url.pathname.split('/').filter((part) => part.length > 0).map(decodeURIComponent);
  const send = (status, type, body, csp) => {
    response.writeHead(status, {
      'Content-Type': type,
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      ...(csp ? { 'Content-Security-Policy': csp } : {}),
    });
    response.end(body);
  };
  if (parts[0] === 'assets' && parts.length === 2) {
    const file = join(dist, 'assets', parts[1]);
    return existsSync(file) && extname(file) === '.js'
      ? send(200, 'text/javascript', readFileSync(file))
      : send(404, 'text/plain', '');
  }
  const book = books.find((candidate) => candidate.id === parts[0]);
  if (book === undefined) {
    return send(404, 'text/plain', '');
  }
  if (parts[1] === 'reader.html') {
    const nonce = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
    // As the app serves it: the nonce on the scripts and on the module preloads.
    const page = readFileSync(join(dist, 'reader.html'), 'utf8')
      .replaceAll('<script', `<script nonce="${nonce}"`)
      .replaceAll('<link rel="modulepreload"', `<link nonce="${nonce}" rel="modulepreload"`);
    return send(200, 'text/html; charset=utf-8', page, policy(`script-src 'nonce-${nonce}'`));
  }
  if (parts[1] === 'pkg') {
    const relative = parts.slice(2).join(sep);
    const file = resolve(book.dir, relative);
    if (!file.startsWith(book.dir + sep) || !existsSync(file)) {
      return send(404, 'text/plain', '');
    }
    return send(200, TYPES[extname(file).toLowerCase()] ?? 'application/octet-stream', readFileSync(file), policy("script-src 'none'"));
  }
  return send(404, 'text/plain', '');
});

/** The page in the app's place: it creates the reader's frame and speaks to it as the app does. */
const HOST_PAGE = `<!doctype html><meta charset="utf-8"><title>host</title>
<style>html,body{margin:0;height:100%;overflow:hidden}iframe{display:block;width:100%;height:100%;border:0}</style>
<script type="module">
const params = new URLSearchParams(location.search);
const id = params.get('book');
const manifest = await (await fetch('/manifest/' + id)).json();
const sections = [];
let before = 0;
manifest.documents.forEach((doc, d) => {
  doc.parts.forEach((part, p) => sections.push({ href: part.href, document: d, part: p, parts: doc.parts.length,
    bytes: part.bytes, textStart: part.textStart, textLength: part.textLength, bookTextStart: before + part.textStart,
    linear: doc.linear, start: part.start }));
  before += doc.textLength;
});
const origin = params.get('origin');
const frame = document.createElement('iframe');
frame.setAttribute('sandbox', 'allow-scripts allow-same-origin');
frame.src = origin + '/' + id + '/reader.html?host=' + location.origin + '&sections=allow-same-origin&package=1&e2e=1';
const state = { opened: false, failed: null, relocations: 0, layouts: [], openMs: 0, last: null, selection: null, selections: 0 };
const started = performance.now();
window.hostE2E = { state, post: (message) => frame.contentWindow.postMessage(message, origin) };
addEventListener('message', (event) => {
  if (event.source !== frame.contentWindow) return;
  const message = event.data;
  if (message.type === 'ready') {
    frame.contentWindow.postMessage({ type: 'open', mode: params.get('mode') ?? 'paginated', animated: true,
      style: { family: 'publisher', size: 100, lineHeight: 1.5, margin: 'normal', theme: 'light' },
      position: params.get('position'), sections, totalText: before, reducedMotion: false }, origin);
  } else if (message.type === 'opened') {
    state.opened = true;
    state.openMs = performance.now() - started;
  } else if (message.type === 'relocate') {
    state.relocations++;
    state.last = message.cfi;
  } else if (message.type === 'layout') {
    state.layouts.push({ ms: message.ms, change: message.change });
  } else if (message.type === 'failed') {
    state.failed = message.message;
  } else if (message.type === 'selection') {
    state.selection = message;
    state.selections++;
  }
});
document.body.append(frame);
</script>`;

const hostServer = http.createServer((request, response) => {
  const url = new URL(request.url, hostOrigin);
  if (url.pathname === '/favicon.ico') {
    // The browser asks for one; the harness has none, and a 404 would read as an error of the reader.
    response.writeHead(204);
    return response.end();
  }
  if (url.pathname === '/host.html') {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    return response.end(HOST_PAGE);
  }
  const manifest = url.pathname.match(/^\/manifest\/([0-9a-f]{64})$/);
  const book = manifest && books.find((candidate) => candidate.id === manifest[1]);
  if (book) {
    response.writeHead(200, { 'Content-Type': 'application/json' });
    return response.end(readFileSync(join(book.dir, 'musaeum.json')));
  }
  response.writeHead(404);
  response.end();
});

await new Promise((ready) => bookServer.listen(0, '127.0.0.1', ready));
await new Promise((ready) => hostServer.listen(0, '127.0.0.1', ready));
bookOrigin = `http://127.0.0.1:${bookServer.address().port}`;
hostOrigin = `http://127.0.0.1:${hostServer.address().port}`;

const profile = mkdtempSync(join(tmpdir(), 'musaeum-reader-check-'));
const debugPort = 30000 + Math.floor(Math.random() * 20000);
// Headless, with a profile of its own: no window opens, and nothing of the person's browser is used.
const browser = spawn(
  chrome,
  [
    '--headless=new',
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profile}`,
    // The GPU stays with whatever the person is doing; layout does not need it.
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--mute-audio',
    '--window-size=1400,900',
    'about:blank',
  ],
  { stdio: 'ignore', windowsHide: true },
);

let pageUrl;
for (let attempt = 0; attempt < 200 && pageUrl === undefined; attempt++) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
    pageUrl = list.find((target) => target.type === 'page')?.webSocketDebuggerUrl;
  } catch {
    // Not listening yet.
  }
  if (pageUrl === undefined) {
    await delay(100);
  }
}
if (pageUrl === undefined) {
  throw new Error('the browser did not start');
}
const page = new Cdp(pageUrl);
await page.connect();
const app = { cdp: page };
// What the page and its frames print as errors, policy violations among them, book by book.
let consoleErrors = [];
/** Blob URLs that failed to load, to look up in the reader that made them before it goes. */
let unexplainedBlobs = [];
let blobReports = [];

/** With --trace-blobs: what each blob URL that failed was, and what revoked it. */
async function explainBlobs(reader) {
  if (!traceBlobs || unexplainedBlobs.length === 0) {
    return;
  }
  await delay(200);
  const urls = unexplainedBlobs;
  unexplainedBlobs = [];
  const found = await reader.evaluate(`${JSON.stringify(urls)}.map((url) => ({ url, ...(window.__musaeumBlob?.(url) ?? { unknown: true }) }))`).catch((error) => [{ error: error.message }]);
  blobReports.push(...found);
}
page.listeners.add(({ method, params }) => {
  if (method === 'Log.entryAdded' && params.entry.level === 'error') {
    const where = params.entry.url ? ` (${params.entry.url})` : '';
    consoleErrors.push(`${params.entry.source}: ${params.entry.text}${where}`.slice(0, 400));
    if (params.entry.url?.startsWith('blob:')) {
      unexplainedBlobs.push(params.entry.url);
    }
  } else if (method === 'Runtime.consoleAPICalled' && params.type === 'error') {
    consoleErrors.push(`console: ${params.args.map((a) => a.value ?? a.description ?? '').join(' ')}`.slice(0, 300));
  } else if (method === 'Runtime.exceptionThrown') {
    consoleErrors.push(`exception: ${params.exceptionDetails.exception?.description ?? params.exceptionDetails.text}`.slice(0, 300));
  }
});
await page.send('Log.enable');

// --trace-blobs: the reader's page notes every blob URL made and revoked, and who did it, so a
// resource that fails to load as revoked can be told apart: a section, a stylesheet, a font.
const traceBlobs = process.argv.includes('--trace-blobs');
if (traceBlobs) {
  await page.send('Page.enable');
  await page.send('Page.addScriptToEvaluateOnNewDocument', {
    source: `(() => {
      if (!location.pathname.endsWith('/reader.html')) return;
      const blobs = new Map();
      const where = () => new Error().stack.split('\\n').slice(2, 9).map((line) => line.trim().replace(/^at /, '').replace(/\\(?https?:[^)]*\\/assets\\//, '(')).join(' < ');
      const create = URL.createObjectURL.bind(URL);
      const revoke = URL.revokeObjectURL.bind(URL);
      URL.createObjectURL = (object) => {
        const url = create(object);
        blobs.set(url, { type: object?.type ?? '', size: object?.size ?? 0, made: Math.round(performance.now()), revoked: null, by: null });
        return url;
      };
      URL.revokeObjectURL = (url) => {
        const entry = blobs.get(url);
        if (entry) { entry.revoked = Math.round(performance.now()); entry.by = where(); }
        return revoke(url);
      };
      window.__musaeumBlob = (url) => blobs.get(url) ?? null;
    })();`,
  });
}

const viewport = (width, height) =>
  page.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });

async function settle(reader) {
  await reader.evaluate('window.musaeumReaderE2E.idle()');
  await reader.evaluate('new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))');
  await delay(450);
  await reader.evaluate('window.musaeumReaderE2E.idle()');
}

function placed(view) {
  if (view.mode === 'continuous') {
    return view.top !== null && Math.abs(view.top) <= Math.max(view.lineHeight, 1) + 0.5;
  }
  return view.onPage === true;
}

const report = { started: new Date().toISOString(), chrome, books: [], failures: [] };
const fail = (message) => {
  report.failures.push(message);
  console.error(`  FAILED: ${message}`);
};
const check = (condition, message) => {
  if (!condition) {
    fail(message);
  }
  return condition;
};

/**
 * Runs in the reader's page: the containers on each side of the cut before section `cut`, with
 * their edges, for a cut that does not look like the space between two blocks.
 */
function seamStructure(cut) {
  const frameOf = (index) => document.querySelector(`.continuous-section[data-section="${index}"] iframe`);
  const describe = (element, frame) => {
    const style = getComputedStyle(element);
    const box = element.getBoundingClientRect();
    return {
      tag: element.localName,
      id: element.id || undefined,
      cls: element.getAttribute('class') || undefined,
      display: style.display,
      margin: `${style.marginTop} ${style.marginBottom}`,
      padding: `${style.paddingTop} ${style.paddingBottom}`,
      border: `${style.borderTopWidth} ${style.borderBottomWidth}`,
      height: style.height,
      top: Math.round(box.top * 10) / 10,
      bottom: Math.round(box.bottom * 10) / 10,
      frameHeight: frame.getBoundingClientRect().height,
      text: (element.textContent ?? '').trim().slice(0, 40),
    };
  };
  const chain = (frame, pick) => {
    const out = [];
    let element = frame?.contentDocument?.body ?? null;
    for (let depth = 0; element !== null && depth < 8; depth++) {
      out.push(describe(element, frame));
      element = pick(element);
    }
    return out;
  };
  const before = frameOf(cut - 1);
  const after = frameOf(cut);
  const blocks = 'p, li, h1, h2, h3, h4, h5, h6, pre, blockquote, figure, table, dl';
  const last = before ? [...before.contentDocument.querySelectorAll(blocks)].at(-1) : null;
  const first = after ? after.contentDocument.querySelector(blocks) : null;
  return {
    before: chain(before, (element) => element.lastElementChild),
    after: chain(after, (element) => element.firstElementChild),
    lastBlock: last
      ? { ...describe(last, before), html: last.outerHTML.slice(0, 300), next: last.nextElementSibling?.outerHTML.slice(0, 200) ?? null }
      : null,
    firstBlock: first
      ? { ...describe(first, after), html: first.outerHTML.slice(0, 300), previous: first.previousElementSibling?.outerHTML.slice(0, 200) ?? null }
      : null,
    frames: [before?.getBoundingClientRect().toJSON(), after?.getBoundingClientRect().toJSON()],
  };
}

/** Opens `book` in a new host page, at `position` (a reader CFI) or at the start, in `mode`. */
async function openReader(book, title, { mode = 'paginated', position = null } = {}) {
  const query = new URLSearchParams({ book: book.id, origin: bookOrigin, mode });
  if (position !== null) {
    query.set('position', position);
  }
  await viewport(1400, 900);
  await page.send('Page.navigate', { url: `${hostOrigin}/host.html?${query}` });
  const state = await waitFor(
    page,
    'window.hostE2E && (window.hostE2E.state.opened || window.hostE2E.state.failed) ? window.hostE2E.state : null',
    { timeoutMs: 60_000, what: `${title} to open` },
  );
  if (state.failed) {
    return { failed: state.failed };
  }
  const reader = await connectFrame(app, bookOrigin, `/${book.id}/reader.html`);
  await waitFor(reader, 'window.musaeumReaderE2E !== undefined', { what: 'the reader hooks' });
  await settle(reader);
  return { reader, openMs: state.openMs };
}

const inspect = (reader) => reader.evaluate('window.musaeumReaderE2E.inspect()');
const post = (message) => page.evaluate(`window.hostE2E.post(${JSON.stringify(message)})`);

async function checkBook(book) {
  const manifest = JSON.parse(readFileSync(join(book.dir, 'musaeum.json'), 'utf8'));
  const title = manifest.metadata?.titles?.[0]?.value ?? book.id.slice(0, 12);
  const result = { id: book.id, title, steps: [], restores: [] };
  console.log(`  ${title}`);
  consoleErrors = [];
  unexplainedBlobs = [];
  blobReports = [];
  const first = await openReader(book, title);
  if (first.failed) {
    fail(`${title}: ${first.failed}`);
    return result;
  }
  result.openMs = first.openMs;
  let reader = first.reader;
  let style = { family: 'publisher', size: 100, lineHeight: 1.5, margin: 'normal', theme: 'light' };
  const setStyle = (change) => {
    style = { ...style, ...change };
    return post({ type: 'set-style', style });
  };

  await reader.evaluate('window.musaeumReaderE2E.goToFraction(0.37)');
  await settle(reader);
  const start = await inspect(reader);
  if (start.anchor === null || !placed(start)) {
    fail(`${title}: no anchor in the middle of the book`);
    return result;
  }
  const anchor = start.anchor;
  const steps = [
    ['font size +20%', () => setStyle({ size: 120 })],
    ['line height 1.7', () => setStyle({ lineHeight: 1.7 })],
    ['wide margins', () => setStyle({ margin: 'wide' })],
    ['serif font', () => setStyle({ family: 'serif' })],
    ['sepia theme', () => setStyle({ theme: 'sepia' })],
    ['screen 1100 × 760', () => viewport(1100, 760)],
    ['screen 1400 × 900', () => viewport(1400, 900)],
    ['continuous', () => post({ type: 'set-mode', mode: 'continuous' })],
    ['font size 110%', () => setStyle({ size: 110 })],
    ['narrow margins', () => setStyle({ margin: 'narrow' })],
    ['book font', () => setStyle({ family: 'publisher' })],
    ['dark theme', () => setStyle({ theme: 'dark' })],
    ['screen 1000 × 720', () => viewport(1000, 720)],
    ['screen 1400 × 900', () => viewport(1400, 900)],
    ['paginated', () => post({ type: 'set-mode', mode: 'paginated' })],
  ];
  for (const [label, action] of steps) {
    await action();
    await settle(reader);
    const view = await inspect(reader);
    const same = view.anchor?.cfi === anchor.cfi;
    result.steps.push({ label, mode: view.mode, sameAnchor: same, placed: placed(view), top: view.top ?? null, left: view.left ?? null, lineHeight: view.lineHeight });
    if (!(same && placed(view))) {
      fail(`${title}: after ${label} (${view.mode}) ${same ? 'the anchor is out of place' : 'the anchor moved'}: ${JSON.stringify({ before: anchor.cfi, after: view.anchor?.cfi, top: view.top, onPage: view.onPage, left: view.left })}`);
    }
  }

  // With the animation off, a page turn moves the anchor to the first character of the next page.
  await post({ type: 'set-animation', animated: false });
  await post({ type: 'turn', step: 1 });
  await settle(reader);
  const turned = await inspect(reader);
  result.turn = { from: anchor.cfi, to: turned.anchor?.cfi ?? null, placed: placed(turned) };
  if (!(turned.anchor !== null && turned.anchor.cfi !== anchor.cfi && placed(turned))) {
    fail(`${title}: the page did not turn: ${JSON.stringify(result.turn)}`);
  }
  result.layouts = (await page.evaluate('window.hostE2E.state.layouts')) ?? [];

  // Opened again at that position, as the app does with the one it kept: the same character.
  const restore = async (expected, mode) => {
    await explainBlobs(reader);
    const again = await openReader(book, title, { mode, position: expected.cfi });
    if (again.failed) {
      fail(`${title}: reopening ${mode} failed: ${again.failed}`);
      return null;
    }
    const view = await inspect(again.reader);
    const exact = view.anchor?.cfi === expected.cfi;
    result.restores.push({ mode, exact, placed: placed(view), top: view.top ?? null, openMs: again.openMs });
    if (!(exact && placed(view) && view.mode === mode)) {
      fail(`${title}: reopened ${view.mode} at ${view.anchor?.cfi}, expected ${mode} at ${expected.cfi}`);
    }
    return again.reader;
  };
  if (turned.anchor !== null) {
    reader = (await restore(turned.anchor, 'paginated')) ?? reader;
  }

  // Continuous, scrolled with the keys, then opened again there.
  await post({ type: 'set-mode', mode: 'continuous' });
  await settle(reader);
  await post({ type: 'key', key: 'PageDown', shift: false });
  await settle(reader);
  await post({ type: 'key', key: 'ArrowDown', shift: false });
  await settle(reader);
  const scrolled = await inspect(reader);
  if (scrolled.mode !== 'continuous' || scrolled.anchor === null) {
    fail(`${title}: no anchor after scrolling`);
  } else {
    reader = (await restore(scrolled.anchor, 'continuous')) ?? reader;
  }

  // Where the normalizer cut a document, the text after the cut is where the uncut document has
  // it: every cut, then again, for a few, after the typography changed.
  const cuts = await reader.evaluate('window.musaeumReaderE2E.cuts()');
  result.cuts = cuts.length;
  result.seams = [];
  const checkSeam = async (cut, typography) => {
    await reader.evaluate(`window.musaeumReaderE2E.goToSection(${cut})`);
    await settle(reader);
    const oracle = await reader.evaluate(`(${seamOracle})(${cut})`, 120_000);
    const seams = await reader.evaluate('window.musaeumReaderE2E.seams()');
    const seam = seams.find((candidate) => candidate.section === cut - 1) ?? null;
    result.seams.push({ typography, ...oracle, gap: seam?.gap ?? null, space: seam?.space ?? null, lead: seam?.lead ?? null });
    if (!oracle.measured || Math.abs(oracle.delta) > SEAM_TOLERANCE_PX) {
      fail(`${title}: after the cut before section ${cut} (${typography}), the text is not where the uncut document has it: ${JSON.stringify(oracle)}`);
      result.seams.at(-1).structure = await reader.evaluate(`(${seamStructure})(${cut})`);
    }
  };
  for (const cut of cuts) {
    await checkSeam(cut, 'book font 100%');
  }
  if (cuts.length > 0) {
    await setStyle({ size: 130, margin: 'wide', lineHeight: 1.35, family: 'serif' });
    await settle(reader);
    for (const cut of cuts.slice(0, 4)) {
      await checkSeam(cut, 'serif 130% wide');
    }
  }
  result.marks = await checkMarks(book, title, cuts);
  // Whatever the book carried, nothing in it ran.
  result.marked = await reader.evaluate(
    "[...document.querySelectorAll('iframe')].filter((frame) => frame.contentDocument?.documentElement?.hasAttribute('data-pwned')).length",
  );
  result.consoleErrors = consoleErrors;
  await explainBlobs(reader);
  result.blobs = blobReports;
  return result;
}

/** A drag with the mouse, as a person selects, from one point of the page to another. */
async function drag(from, to) {
  const event = (type, at, extra = {}) =>
    page.send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, button: 'left', ...extra });
  await event('mouseMoved', from, { buttons: 0 });
  await event('mousePressed', from, { buttons: 1, clickCount: 1 });
  for (let step = 1; step <= 10; step++) {
    const t = step / 10;
    await event('mouseMoved', { x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t }, { buttons: 1 });
    await delay(16);
  }
  await event('mouseReleased', to, { buttons: 0, clickCount: 1 });
}

/** Selecting with the mouse, a mark drawn over it, and the mark kept on its text through relayouts. */
async function checkMarks(book, title, cuts) {
  const result = { selection: null, steps: [], seam: null };
  const opened = await openReader(book, title);
  if (opened.failed) {
    fail(`${title}: reopening for marks: ${opened.failed}`);
    return result;
  }
  const reader = opened.reader;
  let style = { family: 'publisher', size: 100, lineHeight: 1.5, margin: 'normal', theme: 'light' };
  const setStyle = (change) => {
    style = { ...style, ...change };
    return post({ type: 'set-style', style });
  };
  // Somewhere with text on the screen: a book of pictures may have none at the first try.
  let probe = null;
  for (const fraction of [0.4, 0.2, 0.6, 0.8, 0.1]) {
    await reader.evaluate(`window.musaeumReaderE2E.goToFraction(${fraction})`);
    await settle(reader);
    probe = await reader.evaluate('window.musaeumReaderE2E.probeSelection(false)');
    if (probe !== null) {
      break;
    }
  }
  if (probe === null) {
    fail(`${title}: no text on the screen to select`);
    return result;
  }
  const before = (await page.evaluate('window.hostE2E.state.selections')) ?? 0;
  await drag(probe.start, probe.end);
  const selection = await waitFor(
    page,
    `(() => { const s = window.hostE2E.state; return s.selections > ${before} && s.selection?.start ? s.selection : null; })()`,
    { timeoutMs: 5000, what: 'the selection to reach the page' },
  ).catch(() => null);
  const squeeze = (text) => text.replace(/\s+/g, ' ').trim();
  result.selection = { expected: squeeze(probe.text), got: selection === null ? null : squeeze(selection.text), start: selection?.start ?? null, end: selection?.end ?? null };
  if (!check(selection !== null && squeeze(selection.text) === squeeze(probe.text), `${title}: the drag selected ${JSON.stringify(result.selection)}`)) {
    return result;
  }
  const mark = { id: '0192f3a4-5b6c-7d8e-9f01-23456789abcd', color: 'green', kind: 'highlight', note: true, ranges: [{ start: selection.start, end: selection.end }] };
  await post({ type: 'clear-selection' });
  await post({ type: 'marks', marks: [mark] });
  const after = async (label) => {
    await settle(reader);
    const marks = await reader.evaluate('window.musaeumReaderE2E.marks()');
    result.steps.push({ label, drawn: marks.drawn, rects: marks.rects, drift: marks.drift });
    check(marks.drawn === 1 && marks.rects > 0 && marks.drift < 0.5, `${title}: the mark after ${label}: ${JSON.stringify(marks)}`);
  };
  await after('drawn');
  await setStyle({ size: 130 });
  await after('font size 130%');
  await setStyle({ margin: 'wide', family: 'serif' });
  await after('wide margins, serif');
  await post({ type: 'set-mode', mode: 'continuous' });
  await settle(reader);
  await post({ type: 'go-to', target: { cfi: selection.start } });
  await after('continuous');
  await setStyle({ theme: 'dark', size: 100 });
  await after('dark, 100%');

  // Across a cut: a drag from the end of one part into the next, in continuous reading. A cut
  // between pictures, or with only a short line on one side, is not one a person would drag across:
  // a few cuts and positions are tried before giving up.
  if (cuts.length > 0) {
    const section = (cfi) => Number(/^epubcfi\(\/6\/(\d+)/.exec(cfi ?? '')?.[1] ?? -1) / 2 - 1;
    const tries = [];
    attempts: for (const cut of cuts.slice(0, 4)) {
      for (const scroll of [-320, -200, -480]) {
        await reader.evaluate(`window.musaeumReaderE2E.goToSection(${cut})`);
        await settle(reader);
        await page.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 700, y: 450, deltaX: 0, deltaY: scroll });
        await delay(300);
        await settle(reader);
        let seam;
        try {
          seam = await reader.evaluate('window.musaeumReaderE2E.probeSelection(true)');
        } catch (error) {
          tries.push({ cut, scroll, error: error.message });
          continue;
        }
        if (seam === null) {
          tries.push({ cut, scroll, seam: false });
          continue;
        }
        const count = await page.evaluate('window.hostE2E.state.selections');
        await drag(seam.start, seam.end);
        const across = await waitFor(
          page,
          `(() => { const s = window.hostE2E.state; return s.selections > ${count} && s.selection?.start ? s.selection : null; })()`,
          { timeoutMs: 3000, what: 'the selection across the cut' },
        ).catch(() => null);
        const crossed = across !== null && section(across.start) === seam.sections[0] && section(across.end) === seam.sections[1];
        const frames = await reader.evaluate(`[...document.querySelectorAll('iframe')].map((frame) => {
          const selection = frame.contentDocument?.getSelection();
          return selection && !selection.isCollapsed ? { title: frame.title, text: selection.toString().slice(0, 40) } : null;
        }).filter(Boolean)`);
        tries.push({ cut, scroll, seam: true, crossed, start: across?.start ?? null, end: across?.end ?? null, probe: seam, frames });
        if (!crossed) {
          continue;
        }
        result.seam = { sections: seam.sections, start: across.start, end: across.end, text: squeeze(across.text), tries };
        const both = { ...mark, id: '0192f3a4-5b6c-7d8e-9f01-23456789abce', ranges: [{ start: across.start, end: across.end }] };
        await post({ type: 'clear-selection' });
        await post({ type: 'marks', marks: [both] });
        await settle(reader);
        const marks = await reader.evaluate('window.musaeumReaderE2E.marks()');
        result.seam.frames = marks.frames[both.id] ?? 0;
        result.seam.drift = marks.drift;
        check(result.seam.frames === 2 && marks.drift < 0.5, `${title}: the mark across the cut is drawn over ${result.seam.frames} frames, drift ${marks.drift}`);
        break attempts;
      }
    }
    if (result.seam === null) {
      result.seamTries = tries;
      check(false, `${title}: no drag across a cut selected across it: ${JSON.stringify(tries)}`);
    }
  }
  return result;
}

let code = 0;
try {
  for (const book of books) {
    try {
      report.books.push(await checkBook(book));
    } catch (error) {
      fail(`${book.id}: ${error.message}`);
    }
  }
} finally {
  page.close();
  browser.kill();
  bookServer.close();
  hostServer.close();
  await delay(500);
  try {
    rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  } catch {
    // The browser still holds it; the temporary folder goes with the system's cleanup.
  }
  report.finished = new Date().toISOString();
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, JSON.stringify(report, null, 2));
}
if (report.failures.length > 0) {
  console.error(`${report.failures.length} failures`);
  code = 1;
} else {
  const steps = report.books.reduce((sum, book) => sum + book.steps.length, 0);
  console.log(`All checks passed: ${report.books.length} books, ${steps} changes.`);
}
process.exit(code);
