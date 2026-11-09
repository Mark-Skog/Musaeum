#!/usr/bin/env node
// Resolves, in a real browser, the CFIs the Rust engine writes, and writes them again from there.
//
// For each normalized book (a folder with musaeum.json), `cfi-samples` picks positions in every part
// and records the reader's CFI with the text found there. Here each part is served as
// application/xhtml+xml from 127.0.0.1, so the browser's own XML parser builds the DOM, and
// foliate-js's epubcfi.js, the reader's, resolves each CFI and turns the range back into a CFI. A
// match means the same parse, the same counting and the same UTF-16 offsets on both sides.
//
// The browser runs headless, with a profile of its own in %TEMP% that is deleted afterwards: no
// window, nothing of the user's.
//
//   node tools/cfi-browser-check.mjs --cache DIR [--chrome PATH] [--per-part N] [--out REPORT.json]
//
// DIR is a cache folder (it looks under flow/v*/) or a single normalized book.

import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const arg = (name, fallback) => {
  const at = process.argv.indexOf(name);
  return at >= 0 ? process.argv[at + 1] : fallback;
};
const cache = arg('--cache');
const chrome = arg('--chrome', 'C:/Program Files/Google/Chrome/Application/chrome.exe');
const perPart = arg('--per-part', '40');
const out = arg('--out', join(tmpdir(), 'cfi-browser-report.json'));
const samplesExe = join(root, 'target', 'release', 'examples', 'cfi-samples.exe');
if (!cache || !existsSync(cache)) {
  console.error('usage: node tools/cfi-browser-check.mjs --cache DIR');
  process.exit(2);
}
for (const needed of [chrome, samplesExe]) {
  if (!existsSync(needed)) {
    console.error(`${needed} not found (cargo build --release -p musaeum-flow --example cfi-samples)`);
    process.exit(2);
  }
}

/** Every folder under `dir` that holds a normalized book. */
function books(dir) {
  if (existsSync(join(dir, 'musaeum.json'))) return [dir];
  const found = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) found.push(...books(path));
  }
  return found.sort();
}

const types = {
  '.xhtml': 'application/xhtml+xml',
  '.svg': 'image/svg+xml',
  '.css': 'text/css',
  '.js': 'text/javascript',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
};

let current = null;
const epubcfi = readFileSync(join(root, 'vendor', 'foliate-js', 'epubcfi.js'));
const server = createServer((request, response) => {
  const url = decodeURIComponent(new URL(request.url, 'http://x').pathname);
  if (url === '/__epubcfi.js') {
    response.writeHead(200, { 'Content-Type': 'text/javascript' });
    response.end(epubcfi);
    return;
  }
  const path = resolve(current, '.' + url);
  if (!current || !path.startsWith(resolve(current)) || !existsSync(path) || statSync(path).isDirectory()) {
    response.writeHead(404);
    response.end();
    return;
  }
  response.writeHead(200, { 'Content-Type': types[extname(path)] ?? 'application/octet-stream' });
  response.end(readFileSync(path));
});
await new Promise((ready) => server.listen(0, '127.0.0.1', ready));
const serverPort = server.address().port;

const profile = mkdtempSync(join(tmpdir(), 'musaeum-cfi-'));
const debugPort = 30000 + Math.floor(Math.random() * 20000);
const browser = spawn(
  chrome,
  [
    '--headless=new',
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profile}`,
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--mute-audio',
    'about:blank',
  ],
  { stdio: 'ignore', windowsHide: true },
);

async function target() {
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
      const page = list.find((t) => t.type === 'page');
      if (page) return page.webSocketDebuggerUrl;
    } catch {}
    await new Promise((wait) => setTimeout(wait, 100));
  }
  throw new Error('the browser did not start');
}

const socket = new WebSocket(await target());
await new Promise((open) => socket.addEventListener('open', open, { once: true }));
let next = 1;
const pending = new Map();
const events = [];
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve: done, reject } = pending.get(message.id);
    pending.delete(message.id);
    message.error ? reject(new Error(message.error.message)) : done(message.result);
  } else if (message.method) {
    events.push(message.method);
  }
});
function send(method, params = {}) {
  const id = next++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((done, reject) => {
    const timer = setTimeout(() => reject(new Error(`${method} timed out`)), 30_000);
    pending.set(id, {
      resolve: (value) => (clearTimeout(timer), done(value)),
      reject: (error) => (clearTimeout(timer), reject(error)),
    });
  });
}
await send('Page.enable');

async function load(url) {
  events.length = 0;
  await send('Page.navigate', { url });
  for (let waited = 0; waited < 300; waited++) {
    if (events.includes('Page.loadEventFired')) return;
    await new Promise((wait) => setTimeout(wait, 20));
  }
  throw new Error(`${url} did not load`);
}

// What runs in the page: foliate-js's CFI code, as its view uses it, on the browser's own DOM.
const check = async (samples) => {
  const CFI = await import('/__epubcfi.js');
  const parseError = document.getElementsByTagName('parsererror').length > 0;
  return samples.map((sample) => {
    try {
      const parts = CFI.parse(sample.cfi);
      // The package steps go, as foliate's view drops them before resolving in a section.
      parts.shift();
      const range = CFI.toRange(document, parts);
      const node = range.startContainer;
      const offset = range.startOffset;
      if (node.nodeType !== 3) return { why: `not text but ${node.nodeName}`, parseError };
      const back = CFI.fromRange(range);
      return {
        after: node.data.slice(offset, offset + 64),
        before: node.data.slice(Math.max(0, offset - 64), offset),
        back,
        parseError,
      };
    } catch (error) {
      return { why: String(error), parseError };
    }
  });
};

const report = { books: [], samples: 0, text: 0, back: 0, failures: [] };
try {
  for (const book of books(cache)) {
    const samplesPath = join(profile, 'samples.json');
    const made = spawnSync(samplesExe, [book, samplesPath, perPart], { stdio: 'ignore', windowsHide: true });
    if (made.status !== 0) {
      report.failures.push({ book, why: 'cfi-samples failed' });
      continue;
    }
    const samples = JSON.parse(readFileSync(samplesPath, 'utf8'));
    const title = JSON.parse(readFileSync(join(book, 'musaeum.json'), 'utf8')).metadata.titles[0]?.value ?? book;
    const byPart = new Map();
    for (const sample of samples) {
      if (!byPart.has(sample.href)) byPart.set(sample.href, []);
      byPart.get(sample.href).push(sample);
    }
    current = book;
    let text = 0;
    let back = 0;
    for (const [href, list] of byPart) {
      await load(`http://127.0.0.1:${serverPort}/${href}`);
      const result = await send('Runtime.evaluate', {
        expression: `(${check.toString()})(${JSON.stringify(list)})`,
        awaitPromise: true,
        returnByValue: true,
      });
      const answers = result.result.value;
      answers.forEach((answer, index) => {
        const sample = list[index];
        const textMatches = answer.after !== undefined && answer.after.startsWith(sample.after) && answer.before.endsWith(sample.before);
        const path = sample.cfi.slice(sample.cfi.indexOf('!') + 1, -1);
        const backMatches = answer.back === `epubcfi(${path})`;
        if (textMatches) text++;
        if (backMatches) back++;
        if ((!textMatches || !backMatches) && report.failures.length < 40) {
          report.failures.push({ title, href, cfi: sample.cfi, expected: sample.after, answer });
        }
      });
    }
    report.books.push({ title, samples: samples.length, parts: byPart.size, text, back });
    report.samples += samples.length;
    report.text += text;
    report.back += back;
    console.error(`${title}: ${samples.length} CFIs in ${byPart.size} parts; text ${text}, written back ${back}`);
  }
} finally {
  socket.close();
  browser.kill();
  server.close();
  await new Promise((wait) => setTimeout(wait, 500));
  rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
writeFileSync(out, JSON.stringify(report, null, 2));
console.error(`${report.samples} CFIs: ${report.text} found the same text, ${report.back} written back the same; report ${out}`);
process.exit(report.samples > 0 && report.text === report.samples && report.back === report.samples ? 0 : 1);
