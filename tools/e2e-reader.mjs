#!/usr/bin/env node
// Drives the real reader window from outside and checks what a person would see, through the
// WebView2's own debugging protocol (CDP).
//
//   node tools/e2e-reader.mjs [--build target/release] [--fixtures DIR] [--out REPORT.json]
//                             [--corpus-sample FILE] [--require-idle SECONDS] [--skip-visual]
//
// Every click and key goes into the page through Chromium's input pipeline, as trusted events:
// nothing moves the mouse or sends keys to whatever window has the focus, and nothing captures the
// screen; screenshots are of the page itself. The window still appears, so by default the run
// refuses to start unless nobody has touched the machine for a minute.
//
// 1. Reading position: in the real app, with a library, it switches between continuous and paginated
//    reading, with and without the page-turn animation, from the reading bar and from the keyboard,
//    and checks that the Locator (page and the point u, v on it) stays exactly where it was. It then
//    closes the window and opens the document again, and checks that library.db gave back exactly
//    what was saved. For a PDF, a comic and a DjVu book.
// 2. Visual comparison: pages as the window shows them, against the same pages drawn whole by each
//    format's library (tools/visual-check), for the fixtures and a sample of a real collection;
//    and the color page, for RGB and CMYK as PDFium draws them and as the window shows them.
//
// The corpus sample file lists one document per line, as `path|page,page`. Those files are only
// read: the reader opens them read-only, and the library it uses for them is a fresh one in a
// temporary folder, which is removed at the end like everything else this run creates.

import { execFileSync, spawn } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const root = resolve(import.meta.dirname, '..');

function parseArguments(argv) {
  const options = {
    build: join(root, 'target', 'release'),
    fixtures: null,
    out: null,
    corpusSample: null,
    requireIdle: 60,
    skipVisual: false,
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
    else if (flag === '--fixtures') options.fixtures = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--corpus-sample') options.corpusSample = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else if (flag === '--skip-visual') options.skipVisual = true;
    else throw new Error(`unknown argument: ${flag}`);
  }
  return options;
}

/** Seconds since anyone last touched the keyboard or the mouse. */
function idleSeconds() {
  const script =
    "Add-Type -Name IdleE2E -Namespace Musaeum -MemberDefinition '" +
    '[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)] ' +
    'public struct LII { public uint cbSize; public uint dwTime; } ' +
    '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LII p);' +
    "'; $l = New-Object Musaeum.IdleE2E+LII; $l.cbSize = 8; [void][Musaeum.IdleE2E]::GetLastInputInfo([ref]$l); " +
    '[int](([Environment]::TickCount - $l.dwTime) / 1000)';
  // Hidden: on a machine someone is using, no console may flash over what they are doing.
  const output = execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', script], {
    encoding: 'utf8',
    windowsHide: true,
  });
  return Number(output.trim());
}

function checkIdle(options) {
  if (options.requireIdle <= 0) {
    return;
  }
  const idle = idleSeconds();
  if (!(idle >= options.requireIdle)) {
    throw new Error(
      `someone used the machine ${idle} s ago; this opens windows, so it waits for ${options.requireIdle} s of quiet`,
    );
  }
}

/** One connection to a page through the DevTools protocol. */
class Cdp {
  constructor(url) {
    this.url = url;
    this.next = 1;
    this.pending = new Map();
  }

  async connect() {
    this.socket = new WebSocket(this.url);
    await new Promise((resolveOpen, reject) => {
      this.socket.addEventListener('open', resolveOpen, { once: true });
      this.socket.addEventListener('error', () => reject(new Error('no CDP connection')), { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      const waiting = this.pending.get(message.id);
      if (waiting === undefined) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        waiting.reject(new Error(`${message.error.message} (${message.error.code})`));
      } else {
        waiting.resolve(message.result);
      }
    });
  }

  /** Every call has a deadline: an answer that never comes fails the step instead of the whole run. */
  send(method, params = {}, timeoutMs = 30_000) {
    const id = this.next++;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolveSend, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} got no answer in ${timeoutMs} ms`));
      }, timeoutMs);
      const settled = (finish) => (value) => {
        clearTimeout(timer);
        finish(value);
      };
      this.pending.set(id, { resolve: settled(resolveSend), reject: settled(reject) });
    });
  }

  async evaluate(expression) {
    const result = await this.send('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(`page threw: ${JSON.stringify(result.exceptionDetails)}`);
    }
    return result.result.value;
  }

  close() {
    this.socket?.close();
  }
}

let nextPort = 9340;

/** Starts the reader with its debugging port open, and connects to its page. */
async function launch(app, args) {
  const port = nextPort++;
  // The WebView2's profile beside the run's library, as tools/lib/cdp.mjs keeps it.
  const library = args[args.indexOf('--library') + 1];
  const profile = args.includes('--library') && library !== undefined ? { WEBVIEW2_USER_DATA_FOLDER: `${library}-webview2` } : {};
  const child = spawn(app, args, {
    env: {
      ...process.env,
      ...profile,
      // Only for this run: the WebView answers DevTools on this port and draws in sRGB whatever the
      // monitor's profile, so colors can be compared. The app's own arguments are repeated, with
      // the ones its benchmarks use so that a covered window is not slowed down.
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: [
        `--remote-debugging-port=${port}`,
        '--force-color-profile=srgb',
        '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion',
        '--disable-background-timer-throttling',
        '--disable-renderer-backgrounding',
        '--disable-backgrounding-occluded-windows',
      ].join(' '),
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  let log = '';
  child.stderr.on('data', (chunk) => {
    log += chunk;
  });
  const exited = new Promise((resolveExit) => child.on('exit', (code) => resolveExit(code)));
  const deadline = Date.now() + 60_000;
  let target;
  while (target === undefined) {
    if (Date.now() > deadline) {
      child.kill();
      throw new Error(`the reader never offered a page to debug\n${log}`);
    }
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      target = targets.find((candidate) => candidate.type === 'page' && !candidate.url.startsWith('devtools'));
    } catch {
      // Not listening yet.
    }
    if (target === undefined) {
      await delay(200);
    }
  }
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.connect();
  // The page behaves as if it had the focus, without the window being given it.
  await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
  while ((await cdp.evaluate('typeof window.musaeumE2E')) !== 'object') {
    if (Date.now() > deadline) {
      throw new Error(`the reader did not open the document\n${log}`);
    }
    await delay(100);
  }
  return { child, cdp, exited, log: () => log };
}

/** Closes the window the way its close button would, and waits for the program to end. */
async function close(reader) {
  reader.cdp.close();
  execFileSync(
    'powershell',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `$p = Get-Process -Id ${reader.child.pid} -ErrorAction SilentlyContinue; if ($p) { [void]$p.CloseMainWindow() }`,
    ],
    { windowsHide: true },
  );
  const code = await Promise.race([reader.exited, delay(15_000).then(() => 'timeout')]);
  if (code === 'timeout') {
    reader.child.kill();
    throw new Error('the reader did not close when asked');
  }
  return code;
}

const inspect = (cdp) => cdp.evaluate('window.musaeumE2E.inspect()');

/**
 * Waits until nothing moves, the compositor has drawn its last frame, and every page on it is
 * sharp. Without the frame, a page turned without animation would still show the page before.
 */
async function settle(cdp, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let state;
  do {
    state = await inspect(cdp);
    if (!state.moving && !state.framePending && state.sharp) {
      return state;
    }
    await delay(40);
  } while (Date.now() < deadline);
  throw new Error(`the reader did not settle: ${JSON.stringify(state)}`);
}

/** Waits until the last state the page asked to save is the one it is in. */
async function savedAs(cdp, snapshot, timeoutMs = 4_000) {
  const deadline = Date.now() + timeoutMs;
  do {
    const saved = await cdp.evaluate('window.musaeumE2E.saved()');
    const last = saved[saved.length - 1];
    if (last !== undefined && sameSnapshot(last, snapshot)) {
      return last;
    }
    await delay(50);
  } while (Date.now() < deadline);
  throw new Error(`the state was not saved: ${JSON.stringify(snapshot)}`);
}

const FIELDS = ['page', 'u', 'v', 'progress', 'mode', 'pageTurnAnimation', 'zoom', 'spreads'];

function sameSnapshot(a, b) {
  return FIELDS.every((field) => a[field] === b[field]);
}

const sameLocator = (a, b) => a.page === b.page && a.u === b.u && a.v === b.v;

const KEYS = {
  c: { key: 'c', code: 'KeyC', vk: 67, text: 'c' },
  a: { key: 'a', code: 'KeyA', vk: 65, text: 'a' },
  PageDown: { key: 'PageDown', code: 'PageDown', vk: 34 },
  ArrowDown: { key: 'ArrowDown', code: 'ArrowDown', vk: 40 },
  ArrowRight: { key: 'ArrowRight', code: 'ArrowRight', vk: 39 },
  Home: { key: 'Home', code: 'Home', vk: 36 },
};

async function press(cdp, name) {
  const { key, code, vk, text } = KEYS[name];
  await cdp.send('Input.dispatchKeyEvent', {
    type: text === undefined ? 'rawKeyDown' : 'keyDown',
    key,
    code,
    text,
    unmodifiedText: text,
    windowsVirtualKeyCode: vk,
    nativeVirtualKeyCode: vk,
  });
  await cdp.send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key,
    code,
    windowsVirtualKeyCode: vk,
    nativeVirtualKeyCode: vk,
  });
}

async function click(cdp, selector) {
  const point = await cdp.evaluate(`(() => {
    const box = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  })()`);
  for (const type of ['mousePressed', 'mouseReleased']) {
    await cdp.send('Input.dispatchMouseEvent', { type, x: point.x, y: point.y, button: 'left', clickCount: 1 });
  }
}

/** Watches the reader for a while after a key: whether it moved at all, and the last state seen. */
async function watch(cdp, ms = 300) {
  const until = performance.now() + ms;
  let moved = false;
  let state;
  do {
    state = await inspect(cdp);
    moved ||= state.moving;
    await delay(5);
  } while (performance.now() < until);
  return { moved, state };
}

/** Runs one step: an action, then what the reader has to be like afterwards. */
async function step(steps, name, act, check) {
  const started = performance.now();
  try {
    await act();
    const result = await check();
    steps.push({ step: name, passed: true, ms: Math.round(performance.now() - started), ...result });
  } catch (error) {
    steps.push({ step: name, passed: false, error: String(error?.message ?? error) });
  }
}

async function readingPosition(options, app, library, document) {
  const steps = [];
  let reader = await launch(app, ['--document', document, '--library', library, '--e2e']);
  const session = await reader.cdp.evaluate('window.musaeumE2E.session');
  steps.push({
    step: 'open',
    passed: session.documentId !== null,
    format: session.format,
    documentId: session.documentId,
    background: session.background,
    restored: session.readingState,
  });
  const { cdp } = reader;
  let before = await settle(cdp);

  // Somewhere that is not the top of a page, so u and v are not round numbers.
  await step(steps, 'move into the document, continuous', async () => {
    if (before.snapshot.mode !== 'continuous') {
      await click(cdp, '#mode-continuous');
      await settle(cdp);
    }
    await press(cdp, 'PageDown');
    for (let line = 0; line < 3; line++) {
      await press(cdp, 'ArrowDown');
    }
  }, async () => {
    const state = await settle(cdp);
    await savedAs(cdp, state.snapshot);
    before = state;
    return { locator: state.snapshot };
  });

  /** Does `act`, then checks that the Locator did not move and the rest is as `expect` says. */
  const keeps = (name, act, expect) =>
    step(steps, name, act, async () => {
      const state = await settle(cdp);
      const snapshot = state.snapshot;
      for (const [field, value] of Object.entries(expect)) {
        if (snapshot[field] !== value) {
          throw new Error(`${field} is ${snapshot[field]}, expected ${value}`);
        }
      }
      if (!sameLocator(snapshot, before.snapshot)) {
        throw new Error(`the Locator moved from ${JSON.stringify(before.snapshot)} to ${JSON.stringify(snapshot)}`);
      }
      await savedAs(cdp, snapshot);
      before = state;
      return { locator: snapshot };
    });

  await keeps('reading bar: paginated', () => click(cdp, '#mode-paginated'), { mode: 'paginated' });
  if (before.animating) {
    await keeps('reading bar: animation off', () => click(cdp, '#page-turn'), { pageTurnAnimation: false });
  }
  await keeps('C: continuous', () => press(cdp, 'c'), { mode: 'continuous' });
  await keeps('C: paginated, animation still off', () => press(cdp, 'c'), {
    mode: 'paginated',
    pageTurnAnimation: false,
  });
  await keeps('A: animation on', () => press(cdp, 'a'), { pageTurnAnimation: true });

  await step(steps, 'turn a page with the animation', () => press(cdp, 'ArrowRight'), async () => {
    const { moved } = await watch(cdp);
    const state = await settle(cdp);
    if (!moved || state.snapshot.page === before.snapshot.page) {
      throw new Error(`the page should slide to the next one: moved ${moved}, page ${state.snapshot.page}`);
    }
    await savedAs(cdp, state.snapshot);
    before = state;
    return { slid: moved, locator: state.snapshot };
  });
  await keeps('A: animation off', () => press(cdp, 'a'), { pageTurnAnimation: false });
  await step(steps, 'turn a page without the animation', () => press(cdp, 'ArrowRight'), async () => {
    const { moved, state: seen } = await watch(cdp);
    if (moved || seen.snapshot.page === before.snapshot.page) {
      throw new Error(`without the animation the page should turn at once: moved ${moved}`);
    }
    const state = await settle(cdp);
    await savedAs(cdp, state.snapshot);
    before = state;
    return { locator: state.snapshot };
  });
  await keeps('reading bar: continuous', () => click(cdp, '#mode-continuous'), { mode: 'continuous' });

  const last = before.snapshot;
  const exitCode = await close(reader);
  steps.push({ step: 'close', passed: exitCode === 0, exitCode });

  reader = await launch(app, ['--document', document, '--library', library, '--e2e']);
  const reopened = await reader.cdp.evaluate('window.musaeumE2E.session');
  const restored = reopened.readingState;
  const shown = (await settle(reader.cdp)).snapshot;
  const exact = restored !== null && sameSnapshot(restored, last) && sameSnapshot(shown, last);
  steps.push({
    step: 'reopen: library.db gives back exactly what was saved',
    passed: exact,
    saved: last,
    restored,
    shown,
  });
  await close(reader);
  return { document: basename(document), passed: steps.every((entry) => entry.passed), steps };
}

function visualCheck(options, args) {
  const output = execFileSync(join(options.build, 'visual-check.exe'), args, {
    encoding: 'utf8',
    windowsHide: true,
  });
  return JSON.parse(output);
}

async function visualComparison(options, app, library, document, pages, shots, colors) {
  const results = [];
  const reader = await launch(app, ['--document', document, '--library', library, '--e2e']);
  const { cdp } = reader;
  try {
    let state = await settle(cdp);
    // One page at a time, fitted to the window, with no turn to wait for.
    await cdp.evaluate(`(() => {
      const spreads = document.getElementById('spreads');
      spreads.value = 'single';
      spreads.dispatchEvent(new Event('change'));
    })()`);
    if (state.snapshot.mode !== 'paginated') {
      await click(cdp, '#mode-paginated');
    }
    state = await settle(cdp);
    if (state.animating) {
      await press(cdp, 'a');
    }
    const ratio = await cdp.evaluate('window.musaeumE2E.devicePixelRatio');
    const canvas = await cdp.evaluate('window.musaeumE2E.canvas()');
    for (const page of pages) {
      await press(cdp, 'Home');
      for (let turn = 0; turn < page; turn++) {
        await press(cdp, 'ArrowRight');
      }
      state = await settle(cdp);
      const shown = state.pages.find((entry) => entry.page === page);
      if (shown === undefined) {
        const onScreen = state.pages.map((entry) => entry.page).join(', ');
        results.push({
          page,
          passed: false,
          error: `page ${page} is not on the screen: the reader is on page ${state.snapshot.page}, showing ${onScreen}`,
        });
        continue;
      }
      // The reading bar and the counters float over the pages; they are not what is compared.
      await cdp.evaluate(`new Promise((done) => {
        for (const id of ['reading-bar', 'hud']) document.getElementById(id).style.visibility = 'hidden';
        requestAnimationFrame(() => requestAnimationFrame(done));
      })`);
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
      await cdp.evaluate(`(() => {
        for (const id of ['reading-bar', 'hud']) document.getElementById(id).style.visibility = '';
      })()`);
      const shot = join(shots, `${basename(document)}-${page}.png`);
      writeFileSync(shot, Buffer.from(data, 'base64'));
      // The page's corner, from canvas pixels to pixels of the screenshot.
      const x = Math.round(canvas.x * ratio) + shown.x;
      const y = Math.round(canvas.y * ratio) + shown.y;
      const compared = visualCheck(options, [
        'compare',
        '--document', document,
        '--page', String(page),
        '--scale', String(shown.scale),
        '--shot', shot,
        '--x', String(x),
        '--y', String(y),
        '--diff', join(shots, `${basename(document)}-${page}-diff.png`),
      ]);
      const entry = { page, scale: shown.scale, ...compared };
      if (colors) {
        entry.colors = visualCheck(options, [
          'colors',
          '--document', document,
          '--scale', String(shown.scale),
          '--shot', shot,
          '--x', String(x),
          '--y', String(y),
        ]);
      }
      results.push(entry);
    }
  } finally {
    await close(reader);
  }
  return { document, results };
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const app = join(options.build, 'musaeum-desktop.exe');
  const cli = join(options.build, 'musaeum.exe');
  for (const needed of [app, cli, join(options.build, 'visual-check.exe')]) {
    if (!existsSync(needed)) {
      throw new Error(`build it first: ${needed} is missing`);
    }
  }
  const fixtures = options.fixtures ?? join(root, 'fixtures');
  checkIdle(options);

  const work = mkdtempSync(join(tmpdir(), 'musaeum-e2e-'));
  const out = options.out ?? join(root, 'e2e-report.json');
  const report = { startedAt: new Date().toISOString(), reading: [], visual: [] };
  try {
    // A library that knows the documents by their bytes, as the reader's own would.
    const docs = join(work, 'docs');
    const library = join(work, 'library');
    mkdirSync(docs);
    const reading = ['content.pdf', 'comic.cbz', 'book.djvu'];
    for (const name of reading) {
      copyFileSync(join(fixtures, name), join(docs, name));
    }
    execFileSync(cli, ['--library', library, 'root', 'add', docs], { stdio: 'ignore', windowsHide: true });
    execFileSync(cli, ['--library', library, 'reconcile'], { stdio: 'ignore', windowsHide: true });

    for (const name of reading) {
      checkIdle({ ...options, requireIdle: Math.min(options.requireIdle, 30) });
      console.error(`reading position: ${name}`);
      report.reading.push(await readingPosition(options, app, library, join(docs, name)));
    }

    if (!options.skipVisual) {
      // Kept beside the report: the screenshots and, for each, a map of where it differs.
      const shots = join(dirname(out), 'e2e-shots');
      mkdirSync(shots, { recursive: true });
      const blank = join(work, 'empty-library');
      const targets = [
        { document: join(fixtures, 'content.pdf'), pages: [0, 2, 5, 7] },
        { document: join(fixtures, 'colors.pdf'), pages: [0], colors: true },
        { document: join(fixtures, 'mixed.pdf'), pages: [0, 3, 5] },
        { document: join(fixtures, 'comic.cbz'), pages: [0, 2, 3, 6] },
        { document: join(fixtures, 'book.djvu'), pages: [0, 2, 4] },
      ];
      if (options.corpusSample !== null) {
        for (const line of readFileSync(options.corpusSample, 'utf8').split(/\r?\n/)) {
          const [path, pages] = line.split('|');
          if (path?.trim()) {
            targets.push({ document: path.trim(), pages: (pages ?? '0').split(',').map(Number) });
          }
        }
      }
      for (const target of targets) {
        checkIdle({ ...options, requireIdle: Math.min(options.requireIdle, 30) });
        console.error(`visual comparison: ${target.document}`);
        try {
          report.visual.push(
            await visualComparison(options, app, blank, target.document, target.pages, shots, target.colors),
          );
        } catch (error) {
          report.visual.push({ document: target.document, error: String(error?.message ?? error) });
        }
      }
    }
  } finally {
    report.finishedAt = new Date().toISOString();
    writeFileSync(out, JSON.stringify(report, null, 2));
    console.error(`report: ${out}`);
    // The WebView2 under its profile may still be closing (EPERM): tried again for about 16 s.
    rmSync(work, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  }
  const failed = report.reading.filter((entry) => !entry.passed).length;
  console.error(`reading position: ${report.reading.length - failed} of ${report.reading.length} documents passed`);
  process.exitCode = failed === 0 ? 0 : 1;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 2;
});
