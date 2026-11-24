#!/usr/bin/env node
// The installers, tried as a person gets them: installed outside the repository and target/, in a
// temporary folder, run with nothing of the development machine on its PATH, and removed.
//
//   node tools/smoke-bundle.mjs [--bundle target/release/bundle] [--tools target/release]
//                               [--out FILE.json] [--require-idle SECONDS]
//
// 1. Before anything: no Musaeum running (the NSIS installer and uninstaller end a running
//    musaeum-desktop.exe), none installed, and the machine free. The installers' SHA-256.
// 2. The NSIS installer, silent, per user, without shortcuts (/NS), into the temporary folder.
//    Every file the app needs is there: the app, the six workers, pdfium.dll, the por and eng
//    models with the SHA-256 the app pins, LICENSE.txt and THIRD-PARTY-NOTICES.md. No binary asks
//    for the Visual C++ runtime. The version resources say Musaeum 1.0.0.
// 3. The installed app, started from there with only Windows' own folders on its PATH and its
//    library and WebView2 profile in the temporary folder: a comic, a DjVu, an EPUB, a PDF and a
//    scan open; a box on the scan has its text read by the OCR; the annotations go out as JSON-LD,
//    a study report in PDF and a copy of the PDF. While they run, every worker is audited from
//    outside (tools/sandbox-audit): each is the installed one, and each is as locked down as the
//    app's code says. The app closes, and no process of the installation is left.
// 4. The uninstaller, silent: the files and the registry entry go; the folder it keeps for a
//    reinstall (HKCU\Software\Musaeum) is checked and removed by the test.
// 5. Each MSI: its properties and files, read without installing, are the same files; an
//    administrative image (files only, nothing registered) is unpacked, checked the same way, and
//    its app opens a PDF and closes.
// 6. The temporary folder is removed; nothing new is left in %TEMP%; the person's own Musaeum
//    folders (%LOCALAPPDATA%\Musaeum, ...\org.musaeum.desktop) are as they were.

import { execFile, execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

import { click, closeApp, idleSeconds, launch, waitFor } from './lib/cdp.mjs';
import { VC_RUNTIME, importsOf } from './lib/pe.mjs';
import { annotatedEpub, annotatedPdf } from './lib/test-books.mjs';

const root = resolve(import.meta.dirname, '..');
const SCALE_ONE = 4096;
const GENERATED_AT = 1_795_466_405_123;
const VERSION = JSON.parse(readFileSync(join(root, 'apps', 'desktop', 'src-tauri', 'tauri.conf.json'), 'utf8')).version;
const WORKERS = ['musaeum-render-pdf.exe', 'musaeum-pdf-annots.exe', 'musaeum-render-raster.exe', 'musaeum-render-djvu.exe', 'musaeum-ocr.exe', 'musaeum-ingest.exe'];
const SHIPPED = ['musaeum-desktop.exe', ...WORKERS, 'pdfium.dll', 'tessdata\\por.traineddata', 'tessdata\\eng.traineddata', 'tessdata\\LICENSE', 'LICENSE.txt', 'THIRD-PARTY-NOTICES.md'];
const UNINSTALL_KEY = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Musaeum';
// Where the NSIS installer keeps its own settings, by the publisher and the product.
const SETTINGS_KEY = 'HKCU\\Software\\Musaeum';
const START_MENU = join(process.env.APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs', 'Musaeum.lnk');
const DESKTOPS = [join(process.env.USERPROFILE, 'Desktop', 'Musaeum.lnk'), join(process.env.USERPROFILE, 'OneDrive', 'Desktop', 'Musaeum.lnk')];

function parseArguments(argv) {
  const options = {
    bundle: join(root, 'target', 'release', 'bundle'),
    tools: join(root, 'target', 'release'),
    out: join(root, 'bench-results', 'smoke-bundle', 'report.json'),
    requireIdle: 60,
  };
  for (let at = 0; at < argv.length; at++) {
    const flag = argv[at];
    const value = () => argv[++at];
    if (flag === '--bundle') options.bundle = resolve(value());
    else if (flag === '--tools') options.tools = resolve(value());
    else if (flag === '--out') options.out = resolve(value());
    else if (flag === '--require-idle') options.requireIdle = Number(value());
    else throw new Error(`unknown argument: ${flag}`);
  }
  return options;
}

const options = parseArguments(process.argv.slice(2));

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

const report = { started: new Date().toISOString(), options, version: VERSION, installers: [], steps: [], failures: [] };
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

function powershell(command) {
  return execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-Command', command], { encoding: 'utf8', windowsHide: true, maxBuffer: 64 << 20 }).trim();
}

function asArray(text) {
  if (text === '') {
    return [];
  }
  const value = JSON.parse(text);
  return Array.isArray(value) ? value : [value];
}

const quoted = (text) => `'${text.replaceAll("'", "''")}'`;

/** Every process whose executable is inside `folder`. */
function processesUnder(folder) {
  return asArray(
    powershell(
      `Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith(${quoted(folder + '\\')}, [System.StringComparison]::OrdinalIgnoreCase) } | Select-Object ProcessId, Name, ExecutablePath | ConvertTo-Json -Compress`,
    ),
  );
}

/** The WebView2 processes started with a profile inside `folder`. */
function webviewsUsing(folder) {
  return asArray(
    powershell(
      `Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf(${quoted(folder)}, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 } | Select-Object ProcessId | ConvertTo-Json -Compress`,
    ),
  );
}

function running(name) {
  const out = execFileSync('tasklist', ['/FI', `IMAGENAME eq ${name}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8', windowsHide: true });
  return out.split(/\r?\n/).filter((line) => line.toLowerCase().includes(name.toLowerCase())).length;
}

const registryHas = (key) => spawnSync('reg', ['query', key], { windowsHide: true, stdio: 'ignore' }).status === 0;

function versionOf(path) {
  return JSON.parse(powershell(`(Get-Item -LiteralPath ${quoted(path)}).VersionInfo | Select-Object ProductName, ProductVersion, FileVersion, CompanyName, FileDescription, LegalCopyright | ConvertTo-Json -Compress`));
}

/** A folder's entries, one level deep, with their times: enough to tell whether anything touched it. */
function glance(folder) {
  if (!existsSync(folder)) {
    return null;
  }
  return readdirSync(folder)
    .sort()
    .map((name) => {
      const stat = statSync(join(folder, name), { bigint: true });
      return `${name} ${stat.isDirectory() ? 'dir' : String(stat.size)} ${String(stat.mtimeNs)}`;
    });
}

/** Every file under `folder`, by its path inside it. */
function filesUnder(folder) {
  const found = [];
  const walk = (at) => {
    for (const entry of readdirSync(at, { withFileTypes: true })) {
      const path = join(at, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else {
        found.push(relative(folder, path));
      }
    }
  };
  walk(folder);
  return found.sort();
}

function pinnedModels() {
  const source = readFileSync(join(root, 'crates', 'musaeum-ocr', 'src', 'models.rs'), 'utf8');
  return new Map([...source.matchAll(/code: "([a-z_]+)",[\s\S]*?sha256: "([0-9a-f]{64})"/g)].map(([, code, hash]) => [code, hash]));
}

/** What a set of installed files has to be: complete, free of the Visual C++ runtime, versioned. */
function checkFiles(folder, label) {
  const files = filesUnder(folder);
  const missing = SHIPPED.filter((name) => !files.some((file) => file.toLowerCase() === name.toLowerCase()));
  step(`${label}: every file the app needs is there`, missing.length === 0, { missing, files });
  const binaries = files.filter((file) => /\.(exe|dll)$/i.test(file) && !/^uninstall\.exe$/i.test(file));
  const foreign = {};
  for (const file of binaries) {
    const asked = importsOf(join(folder, file)).filter((dll) => VC_RUNTIME.test(dll) || (dll !== 'pdfium.dll' && !dll.startsWith('api-ms-win-') && !existsSync(join(process.env.SystemRoot, 'System32', dll))));
    if (asked.length > 0) {
      foreign[file] = asked;
    }
  }
  step(`${label}: no binary asks for the Visual C++ runtime or any DLL Windows lacks`, Object.keys(foreign).length === 0, { foreign, binaries });
  const pinned = pinnedModels();
  const models = ['por', 'eng'].map((code) => ({ code, ok: existsSync(join(folder, 'tessdata', `${code}.traineddata`)) && sha256(join(folder, 'tessdata', `${code}.traineddata`)) === pinned.get(code) }));
  step(`${label}: the por and eng models have the SHA-256 the app pins`, models.every((model) => model.ok), { models });
  const notices = existsSync(join(folder, 'THIRD-PARTY-NOTICES.md')) && sha256(join(folder, 'THIRD-PARTY-NOTICES.md')) === sha256(join(root, 'THIRD-PARTY-NOTICES.md'));
  step(`${label}: THIRD-PARTY-NOTICES.md is the repository's`, notices);
  const app = versionOf(join(folder, 'musaeum-desktop.exe'));
  step(`${label}: the app's version resource says Musaeum ${VERSION}`, app.ProductName === 'Musaeum' && app.ProductVersion?.startsWith(VERSION) && app.FileVersion?.startsWith(VERSION), { app });
  return { files, hashes: Object.fromEntries(files.map((file) => [file, sha256(join(folder, file))])) };
}

// --- Driving the installed app, as the window tests do ---

async function rows(main, count) {
  return (
    await waitFor(
      main,
      `(() => { const r = window.musaeumApp.rows(); const i = window.musaeumApp.library(); return r.total === ${count} && i && !i.indexing && r.rows.every((b) => b.blake3 !== null) ? r.rows : null; })()`,
      { timeoutMs: 180_000, what: `${count} books in the library` },
    )
  ).slice();
}

async function openBook(main, documentId) {
  checkStillQuiet();
  await waitFor(main, `document.querySelector('[data-document="${documentId}"]') !== null`, { timeoutMs: 20_000, what: `the card of document ${documentId}` });
  await click(main, `[data-document="${documentId}"]`);
  await delay(300);
  if ((await main.evaluate("window.musaeumApp.view()")) !== 'reader') {
    await main.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await main.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
  }
  return waitFor(main, `(() => { const r = window.musaeumApp.reader(); return r !== null && r.documentId === ${documentId} && (r.ready || r.error !== null) ? r : null; })()`, {
    timeoutMs: 90_000,
    what: `document ${documentId} to open`,
  });
}

async function back(main) {
  await click(main, '#back-to-library');
  await waitFor(main, "window.musaeumApp.view() === 'library' && document.querySelector('.library') !== null", { timeoutMs: 20_000, what: 'the library' });
}

async function waitSharp(main) {
  return waitFor(main, '(async () => { const i = await window.musaeumApp.inspectFixed(); return i && i.sharp && !i.framePending && !i.moving ? i : null; })()', {
    timeoutMs: 60_000,
    what: 'the pages to be drawn sharp',
  });
}

async function screenOf(main, page, u, v) {
  const inspection = await main.evaluate('window.musaeumApp.inspectFixed()');
  const placed = inspection.pages.find((candidate) => candidate.page === page);
  const { canvas, dpr } = await main.evaluate('window.musaeumApp.rects()');
  const width = placed.widthPt * (placed.scale / SCALE_ONE);
  const height = placed.heightPt * (placed.scale / SCALE_ONE);
  return { x: canvas.x + (placed.x + u * width) / dpr, y: canvas.y + (placed.y + v * height) / dpr };
}

async function drag(main, from, to) {
  checkStillQuiet();
  const mouse = (type, at, extra) => main.send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, button: type === 'mouseMoved' ? 'none' : 'left', ...extra });
  await mouse('mouseMoved', from, { buttons: 0 });
  await mouse('mousePressed', from, { buttons: 1, clickCount: 1 });
  for (let n = 1; n <= 12; n++) {
    const t = n / 12;
    await main.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t, button: 'left', buttons: 1 });
    await delay(16);
  }
  await mouse('mouseReleased', to, { buttons: 0, clickCount: 1 });
}

/** Runs sandbox-audit on the workers running from `folder` now. */
function audit(folder, required, name) {
  const out = join(dirname(options.out), `sandbox-${name}.json`);
  mkdirSync(dirname(out), { recursive: true });
  const run = spawnSync(join(options.tools, 'sandbox-audit.exe'), ['--under', folder, '--require', required.join('+'), '--out', out], { windowsHide: true, encoding: 'utf8' });
  const result = existsSync(out) ? JSON.parse(readFileSync(out, 'utf8')) : { error: run.stderr };
  return { passed: run.status === 0, result };
}

/**
 * Runs `work` while `sandbox-audit --follow-ms` looks, over and over, at the worker `exe` running
 * from `folder`: for a worker the host starts for one request and ends after it. Every look comes
 * back, each with how old the process was.
 */
async function followDuring(folder, exe, name, work) {
  const out = join(dirname(options.out), `sandbox-${name}.json`);
  mkdirSync(dirname(out), { recursive: true });
  const watcher = new Promise((resolveRun) =>
    execFile(join(options.tools, 'sandbox-audit.exe'), ['--under', folder, '--only', exe, '--follow-ms', '20000', '--out', out], { windowsHide: true }, () => resolveRun()),
  );
  // The audit is looking before the request starts.
  await delay(300);
  const result = await work();
  await watcher;
  const looks = existsSync(out) ? (JSON.parse(readFileSync(out, 'utf8')).workers ?? []) : [];
  return { result, looks };
}

/**
 * What the owner of a handle is: its session, the services it runs, and its executable, from the
 * audit's own reading of the handle when there is one (a process of SYSTEM does not tell a user its
 * path), else from Windows, else from the command line its service was registered with.
 */
function processInfo(pid, image) {
  const text = powershell(
    `$p = Get-CimInstance Win32_Process -Filter "ProcessId = ${Number(pid)}"; $s = @(Get-CimInstance Win32_Service -Filter "ProcessId = ${Number(pid)}"); [pscustomobject]@{ session = $p.SessionId; executable = $p.ExecutablePath; services = @($s | ForEach-Object { $_.Name }); registered = @($s | ForEach-Object { $_.PathName }) } | ConvertTo-Json -Compress`,
  );
  if (text === '') {
    return null;
  }
  const info = JSON.parse(text);
  const fromService = (info.registered ?? []).map((line) => (line.match(/^"([^"]+)"/) ?? line.match(/^(\S+)/))?.[1]).find(Boolean) ?? null;
  return { ...info, path: image ?? info.executable ?? fromService, fromService };
}

/**
 * Sorts the looks at a worker that lives for one request by the line the sandbox itself draws: its
 * lockdown. Before it, the worker loads what it needs with its startup token and reads nothing from
 * the host; when it enters it, it drops that token for good, and only then reads a book. Every look
 * after that has to pass. What is seen before is not passed over: each handle on another process
 * and each DLL from outside Windows and the installation has to be traced to a service of this
 * machine, outside both, and the rest is listed, as what the startup holds until the lockdown
 * closes it.
 */
function judgeFollowed(looks, install) {
  // A look counts as the startup only on positive evidence: a thread seen holding the startup token,
  // or a loader that has not run yet. Anything else, an unknown included, has to pass.
  const starts = (look) => look.impersonating === true || !Array.isArray(look.modulesOutsideWindows);
  const locked = (look) => !starts(look);
  const working = looks.filter(locked);
  const starting = looks.filter((look) => !locked(look));
  const early = starting.filter((look) => (look.violations ?? []).length > 0);
  const owners = new Map();
  const dlls = new Set();
  const startup = new Set();
  for (const look of early) {
    for (const violation of look.violations) {
      const owner = violation.match(/owner: Some\((\d+)\)/);
      const ownerImage = violation.match(/owner_image: Some\("((?:[^"\\]|\\.)+)"\)/);
      const dll = violation.match(/^a DLL from outside Windows and the app's folder: (.+)$/);
      if (violation.startsWith('a handle on another process') && owner) {
        owners.set(Number(owner[1]), ownerImage ? ownerImage[1].replaceAll('\\\\', '\\') : null);
      } else if (dll) {
        dlls.add(dll[1]);
      } else {
        startup.add(violation.replace(/HeldHandle \{.*\}/, (text) => text.replace(/, owner: None, owner_image: None/, '')));
      }
    }
  }
  for (const [pid, image] of owners) {
    owners.set(pid, processInfo(pid, image));
  }
  const vendors = [...owners.values()].filter((info) => info !== null && info.session === 0 && info.services.length > 0 && info.path && !info.path.toLowerCase().startsWith(install.toLowerCase()) && !info.path.toLowerCase().startsWith(process.env.SystemRoot.toLowerCase()));
  const vendorFolder = (path) => path.toLowerCase().split('\\').slice(0, 3).join('\\');
  const folders = new Set(vendors.map((info) => vendorFolder(info.path)));
  // A DLL with no handle to say whose it is: a running service of the same product (its executable
  // in a folder of the same name) has to be there.
  const services = new Map();
  for (const dll of dlls) {
    const product = vendorFolder(dll).split('\\').at(-1);
    if (![...folders].some((folder) => folder.endsWith(`\\${product}`))) {
      const found = asArray(
        powershell(
          `Get-CimInstance Win32_Service | Where-Object { $_.State -eq 'Running' -and $_.PathName -and $_.PathName.ToLower().Contains(${quoted(`\\${product}\\`)}) } | Select-Object Name, PathName, ProcessId | ConvertTo-Json -Compress`,
        ),
      );
      services.set(dll, found);
    }
  }
  const explained =
    owners.size === vendors.length &&
    [...dlls].every((dll) => [...folders].some((folder) => folder.endsWith(`\\${vendorFolder(dll).split('\\').at(-1)}`)) || (services.get(dll)?.length ?? 0) > 0);
  return {
    working,
    starting,
    early,
    clean: working.length > 0 && working.every((look) => (look.violations ?? []).length === 0 && look.image?.toLowerCase().startsWith(install.toLowerCase())),
    environment: early.length === 0 ? null : { explained, owners: Object.fromEntries(owners), dlls: [...dlls], services: Object.fromEntries(services), beforeTheLockdown: [...startup] },
  };
}

/** A minimal environment: Windows' own folders on the PATH, nothing of Rust, Node or the repository. */
function cleanEnvironment() {
  const saved = { ...process.env };
  const system = process.env.SystemRoot;
  for (const key of Object.keys(process.env)) {
    if (/^(CARGO|RUST|NODE|NPM|npm_|VSCODE|PYTHON|VCPKG|INCLUDE$|LIB$|LIBPATH$)/i.test(key)) {
      delete process.env[key];
    }
  }
  process.env.PATH = [`${system}\\System32`, system, `${system}\\System32\\Wbem`, `${system}\\System32\\WindowsPowerShell\\v1.0`].join(';');
  return () => {
    for (const key of Object.keys(process.env)) {
      delete process.env[key];
    }
    Object.assign(process.env, saved);
  };
}

async function runInstalledApp(install, work) {
  const shelf = join(work, 'estante');
  const out = join(work, 'saida');
  const generated = join(work, 'gerados');
  for (const folder of [shelf, out, generated]) {
    mkdirSync(folder, { recursive: true });
  }
  writeFileSync(join(shelf, 'livro.pdf'), annotatedPdf().v1);
  writeFileSync(join(shelf, 'livro.epub'), annotatedEpub().v1);
  for (const only of ['comics', 'djvu', 'ocr']) {
    execFileSync(join(options.tools, 'gen-fixtures.exe'), ['--only', only, '--out', generated], { windowsHide: true });
  }
  copyFileSync(join(generated, 'comic.cbz'), join(shelf, 'quadrinho.cbz'));
  copyFileSync(join(generated, 'book.djvu'), join(shelf, 'livro.djvu'));
  execFileSync(join(options.tools, 'visual-check.exe'), ['scan', '--document', join(generated, 'ocr-native.pdf'), '--out', join(shelf, 'escaneado.pdf'), '--dpi', '300', '--noise', '1', '--seed', '1'], { stdio: 'ignore', windowsHide: true });
  const truth = JSON.parse(readFileSync(join(generated, 'ocr-native.json'), 'utf8'));

  const app = join(install, 'musaeum-desktop.exe');
  const restore = cleanEnvironment();
  let session;
  try {
    session = await launch(app, ['--library', join(work, 'biblioteca'), '--e2e', '--width', '1400', '--height', '950'], 'window.musaeumApp !== undefined');
  } finally {
    restore();
  }
  const main = session.cdp;
  const audits = [];
  try {
    await waitFor(main, "document.querySelector('.welcome') !== null || document.querySelector('.library') !== null", { what: 'the first screen' });
    step('the installed app opens its window, from its own folder, with a clean PATH', true, { app });
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const books = await rows(main, 5);
    const id = (name) => books.find((row) => basename(row.path) === name)?.documentId;
    step('the five books come into the library, each read by the installed ingest worker', books.length === 5, { books: books.map((row) => ({ name: basename(row.path), format: row.format })) });

    for (const [name, worker] of [
      ['quadrinho.cbz', 'musaeum-render-raster.exe'],
      ['livro.djvu', 'musaeum-render-djvu.exe'],
    ]) {
      const state = await openBook(main, id(name));
      const sharp = state.error === null ? await waitSharp(main).then(() => true, () => false) : false;
      step(`${name} opens and is drawn`, state.error === null && sharp, { error: state.error });
      const checked = audit(install, [worker], basename(name, '.' + name.split('.').at(-1)));
      audits.push(checked.result);
      step(`while ${name} is open, its worker is the installed one and as locked down as the app says`, checked.passed, { audit: checked.result });
      await back(main);
    }
    const epub = await openBook(main, id('livro.epub'));
    step('the EPUB opens in the fluid reader, normalized by the installed worker', epub.error === null && epub.ready, { error: epub.error, kind: epub.kind });
    await back(main);
    const pdf = await openBook(main, id('livro.pdf'));
    step('the PDF opens and is drawn by the installed PDFium', pdf.error === null && (await waitSharp(main).then(() => true, () => false)), { error: pdf.error });
    await back(main);

    // The scan: a box over three lines, read by the installed OCR with the installed models.
    const scan = await openBook(main, id('escaneado.pdf'));
    await waitSharp(main);
    const boxed = truth.lines.filter((line) => line.page === 0 && line.kind === 'body' && line.block === 2 && line.text !== '').slice(0, 3);
    const size = truth.page;
    const corner = ([x, y]) => screenOf(main, 0, x / size[0], 1 - y / size[1]);
    await click(main, 'button[aria-label="Caixa retangular"]');
    await waitSharp(main);
    await drag(main, await corner([64, boxed[0].start[1] + boxed[0].size * 0.9]), await corner([552, boxed[2].start[1] - boxed[2].size * 0.4]));
    await waitFor(main, "(() => { const s = window.musaeumApp.reader()?.selection; return s && s.kind === 'region' && !s.reading ? s : null; })()", { timeoutMs: 120_000, what: 'the text under the box' });
    await click(main, '#mark-color-purple');
    const ocr = await waitFor(main, "(() => { const a = window.musaeumApp.reader()?.annotations ?? []; return a.length === 1 && a[0].quote !== null && a[0].quoteSource === 'ocr' ? a[0] : null; })()", {
      timeoutMs: 180_000,
      what: 'the quote read by the OCR',
    }).catch(() => null);
    step('a box on the scan has its text read by the installed OCR', scan.error === null && ocr !== null && boxed.every((line) => ocr.quote.includes(line.text.split(' ')[0])), { quote: ocr?.quote, lines: boxed.map((line) => line.text) });

    // The panel's actions, as a person reaches them.
    await click(main, '#annotations-toggle');
    await waitFor(main, "['#exchange-export-jsonld', '#exchange-report-pdf', '#pdf-export-pdf'].every((id) => document.querySelector(id) !== null)", { timeoutMs: 10_000, what: 'the annotations panel' });
    const files = { jsonld: join(out, 'anotacoes.jsonld'), report: join(out, 'relatorio.pdf'), copy: join(out, 'escaneado (anotado).pdf') };
    const jsonld = await waitFor(main, `window.musaeumApp.exchange('reader', 'export-jsonld', ${JSON.stringify(files.jsonld)})`, { timeoutMs: 120_000, what: 'the JSON-LD' });
    const items = jsonld.ok && existsSync(files.jsonld) ? JSON.parse(readFileSync(files.jsonld, 'utf8')).first?.items ?? [] : [];
    step('the annotation goes out as JSON-LD', items.length === 1, { jsonld });
    const written = await waitFor(main, `window.musaeumApp.exchange('reader', 'report-pdf', ${JSON.stringify(files.report)}, ${GENERATED_AT})`, { timeoutMs: 120_000, what: 'the PDF report' });
    const text = written.ok ? (await main.evaluate(`window.musaeumApp.pdfText(${JSON.stringify(files.report)})`)).join(' ') : '';
    step('the study report in PDF is written with the embedded fonts and read back by PDFium', written.ok && text.includes('página 1 de') && text.includes(`Musaeum ${VERSION}`), { written, text: text.slice(0, 400) });
    // The annotations worker lives for one request: it is audited while the copy is written.
    const exported = await followDuring(install, 'musaeum-pdf-annots.exe', 'annots', () =>
      waitFor(main, `window.musaeumApp.pdf('export-pdf', ${JSON.stringify(files.copy)})`, { timeoutMs: 180_000, what: 'the PDF copy' }),
    );
    const copied = exported.result;
    const found = copied.ok ? await main.evaluate(`window.musaeumApp.pdfAnnotations(${JSON.stringify(files.copy)})`) : null;
    step('a copy of the scan with the annotation in it, written by the installed annotations worker', copied.ok && (found?.reading?.annots?.length ?? 0) >= 1, { copied, annots: found?.reading?.annots?.length });
    const judged = judgeFollowed(exported.looks, install);
    step('from its lockdown until it ends, the annotations worker is the installed one and as locked down as the app says', judged.clean, {
      looks: exported.looks.length,
      beforeTheLockdown: judged.starting.length,
      afterTheLockdown: judged.working.map((look) => ({ pid: look.pid, ageMs: look.ageMs, violations: look.violations })),
    });
    if (judged.environment !== null) {
      report.environment = judged.environment;
      step('before its lockdown, every handle on another process and every DLL from outside came from a service of this machine, outside Windows and the installation', judged.environment.explained, {
        environment: judged.environment,
        starting: judged.early.map((look) => ({ pid: look.pid, ageMs: look.ageMs, violations: look.violations, modules: look.modulesOutsideWindows })),
      });
    }
    audits.push({ workers: judged.working });

    const checked = audit(install, ['musaeum-render-pdf.exe', 'musaeum-ocr.exe', 'musaeum-ingest.exe'], 'pdf');
    audits.push(checked.result);
    step('every worker running now is the installed one and as locked down as the app says', checked.passed, { audit: checked.result });
    const seen = new Set(audits.flatMap((result) => (result.workers ?? []).map((worker) => worker.exe.toLowerCase())));
    step('the six workers were all seen running from the installation, and all passed', WORKERS.every((worker) => seen.has(worker)), { seen: [...seen] });
    report.console = session.console;
  } finally {
    await closeApp(session).catch((error) => step('the installed app closes when asked', false, { error: String(error) }));
  }
  await delay(2000);
  const left = processesUnder(install);
  const webviews = webviewsUsing(join(work, 'biblioteca'));
  step('no process of the installation is left, nor a WebView2 of its profile', left.length === 0 && webviews.length === 0, { left, webviews });
}

/** Opens an app unpacked from an MSI on a PDF, and closes it. */
async function quickOpen(folder, work) {
  const shelf = join(work, 'estante-msi');
  mkdirSync(shelf, { recursive: true });
  writeFileSync(join(shelf, 'livro.pdf'), annotatedPdf().v1);
  const restore = cleanEnvironment();
  let session;
  try {
    session = await launch(join(folder, 'musaeum-desktop.exe'), ['--library', join(work, 'biblioteca-msi'), '--e2e', '--width', '1200', '--height', '800'], 'window.musaeumApp !== undefined');
  } finally {
    restore();
  }
  const main = session.cdp;
  try {
    await main.evaluate(`window.musaeumApp.addRoot(${JSON.stringify(shelf)})`);
    const [book] = await rows(main, 1);
    const state = await openBook(main, book.documentId);
    step('the app from the MSI opens a PDF and draws it', state.error === null && (await waitSharp(main).then(() => true, () => false)), { error: state.error });
  } finally {
    await closeApp(session).catch((error) => step('the app from the MSI closes when asked', false, { error: String(error) }));
  }
  await delay(2000);
  step('no process of the MSI image is left', processesUnder(folder).length === 0);
}

async function main() {
  const nsis = join(options.bundle, 'nsis', `Musaeum_${VERSION}_x64-setup.exe`);
  const msis = existsSync(join(options.bundle, 'msi')) ? readdirSync(join(options.bundle, 'msi')).filter((name) => name.endsWith('.msi')).map((name) => join(options.bundle, 'msi', name)) : [];
  for (const path of [nsis, join(options.tools, 'sandbox-audit.exe'), join(options.tools, 'gen-fixtures.exe'), join(options.tools, 'visual-check.exe')]) {
    if (!existsSync(path)) {
      throw new Error(`${path} is missing: run npm run build:bundle and cargo build --release -p sandbox-audit -p visual-check first`);
    }
  }
  if (running('musaeum-desktop.exe') > 0) {
    throw new Error('a Musaeum is running; the installer would end it, so this waits until it is closed');
  }
  if (registryHas(UNINSTALL_KEY)) {
    throw new Error('Musaeum is installed for this user; this tries the installer on a machine without it');
  }
  if (options.requireIdle > 0) {
    const idle = idleSeconds();
    if (idle < options.requireIdle) {
      throw new Interrupted(`someone used the machine ${idle} s ago; this opens windows, so it waits for ${options.requireIdle} s of quiet`);
    }
    quietSince = idle;
  }
  for (const path of [nsis, ...msis]) {
    report.installers.push({ path, bytes: statSync(path).size, sha256: sha256(path) });
  }
  const local = process.env.LOCALAPPDATA;
  const own = [join(local, 'Musaeum'), join(local, 'org.musaeum.desktop'), join(process.env.APPDATA, 'org.musaeum.desktop'), join(process.env.APPDATA, 'Musaeum')];
  const ownBefore = own.map(glance);
  const tempBefore = new Set(readdirSync(tmpdir()));
  const startMenuBefore = existsSync(START_MENU);
  const settingsBefore = registryHas(SETTINGS_KEY);
  const desktopsBefore = DESKTOPS.map((path) => existsSync(path));

  const work = mkdtempSync(join(tmpdir(), 'musaeum-smoke-'));
  report.work = work;
  try {
    // --- 2. NSIS ---
    console.log('NSIS');
    const install = join(work, 'instalado');
    const installed = spawnSync(nsis, ['/S', '/NS', `/D=${install}`], { windowsHide: true, timeout: 300_000 });
    step('the NSIS installer finishes silently, per user, without elevation', installed.status === 0 && existsSync(join(install, 'musaeum-desktop.exe')), { status: installed.status, error: installed.error?.message });
    // `/NS` leaves out both shortcuts, the Start menu's and the desktop's: nothing on the person's
    // screens changes for a test.
    step('it registers itself for this user, and with /NS makes no shortcut', registryHas(UNINSTALL_KEY) && existsSync(START_MENU) === startMenuBefore && DESKTOPS.every((path, at) => existsSync(path) === desktopsBefore[at]), {
      uninstallKey: registryHas(UNINSTALL_KEY),
      startMenu: existsSync(START_MENU),
    });
    const nsisFiles = checkFiles(install, 'NSIS');
    report.installed = nsisFiles.files;

    // --- 3. The installed app ---
    console.log('the installed app');
    await runInstalledApp(install, work);

    // --- 4. Uninstalling ---
    console.log('uninstalling');
    const uninstaller = join(install, 'uninstall.exe');
    // Run where it is (`_?=`), so that it does not copy itself to %TEMP% and returns when done.
    const removed = spawnSync(uninstaller, ['/S', `_?=${install}`], { windowsHide: true, timeout: 300_000 });
    const leftover = existsSync(install) ? filesUnder(install) : [];
    step('the uninstaller removes every file but itself, the registry entry and the Start menu shortcut', removed.status === 0 && leftover.every((file) => file.toLowerCase() === 'uninstall.exe') && !registryHas(UNINSTALL_KEY) && existsSync(START_MENU) === startMenuBefore, {
      status: removed.status,
      leftover,
    });
    rmSync(install, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
    // A silent uninstall keeps where the app was installed (HKCU\Software\Musaeum\Musaeum), so that
    // installing again goes to the same place; only the interactive "delete the app's data" removes
    // it. The test made it, so the test removes it, after checking it holds nothing else.
    if (!settingsBefore && registryHas(SETTINGS_KEY)) {
      const kept = execFileSync('reg', ['query', SETTINGS_KEY, '/s'], { encoding: 'utf8', windowsHide: true });
      const values = kept.split(/\r?\n/).filter((line) => /REG_/.test(line)).map((line) => line.trim());
      const onlyTheFolder = values.length === 1 && values[0].startsWith('(Default)') && values[0].toLowerCase().endsWith(install.toLowerCase());
      step('a silent uninstall leaves only the folder it was installed in, under HKCU\\Software\\Musaeum, which the test removes', onlyTheFolder, { values });
      if (onlyTheFolder) {
        execFileSync('reg', ['delete', SETTINGS_KEY, '/f'], { windowsHide: true });
      }
    }

    // --- 5. MSI ---
    for (const msi of msis) {
      const label = `MSI ${basename(msi)}`;
      console.log(label);
      const tables = JSON.parse(execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', join(root, 'tools', 'lib', 'msi-tables.ps1'), '-Path', msi], { encoding: 'utf8', windowsHide: true }));
      const properties = tables.properties;
      step(`${label}: Musaeum ${VERSION}, by Musaeum, with an upgrade code`, properties.ProductName === 'Musaeum' && properties.ProductVersion === VERSION && properties.Manufacturer === 'Musaeum' && /^\{[0-9A-F-]{36}\}$/i.test(properties.UpgradeCode ?? ''), {
        properties: { ProductName: properties.ProductName, ProductVersion: properties.ProductVersion, Manufacturer: properties.Manufacturer, UpgradeCode: properties.UpgradeCode, ProductLanguage: properties.ProductLanguage, ALLUSERS: properties.ALLUSERS },
      });
      report.installers.find((entry) => entry.path === msi).upgradeCode = properties.UpgradeCode;
      const named = new Map(tables.files.map((file) => [file.name.toLowerCase(), file.size]));
      const differs = SHIPPED.filter((name) => {
        const file = basename(name.replaceAll('\\', '/')).toLowerCase();
        return !named.has(file) || (file !== 'musaeum-desktop.exe' && named.get(file) !== statSync(join(root, 'target', 'bundle-stage', name)).size);
      });
      step(`${label}: its File table holds the same files as the NSIS installer, of the same sizes`, differs.length === 0, { differs, files: tables.files });
      const image = join(work, `msi-${basename(msi, '.msi')}`);
      const unpacked = spawnSync('msiexec', ['/a', msi, '/qn', `TARGETDIR=${image}`], { windowsHide: true, timeout: 300_000 });
      const exe = existsSync(image) ? filesUnder(image).find((file) => basename(file).toLowerCase() === 'musaeum-desktop.exe') : undefined;
      if (step(`${label}: an administrative image unpacks, without installing anything`, unpacked.status === 0 && exe !== undefined, { status: unpacked.status })) {
        const folder = join(image, dirname(exe));
        const msiFiles = checkFiles(folder, label);
        const same = Object.entries(nsisFiles.hashes).filter(([file]) => file.toLowerCase() !== 'musaeum-desktop.exe' && file.toLowerCase() !== 'uninstall.exe').every(([file, hash]) => msiFiles.hashes[file] === hash);
        step(`${label}: every file but the app's executable is byte for byte the NSIS installer's`, same);
        await quickOpen(folder, work);
      }
      step(`${label}: nothing was installed for this user or the machine`, !registryHas(UNINSTALL_KEY));
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
    const appeared = readdirSync(tmpdir()).filter((name) => !tempBefore.has(name));
    report.newInTemp = appeared;
    step('the temporary folder is gone, and nothing of the installers or the app is left in %TEMP%', report.leftInTemp === null && !appeared.some((name) => /musaeum|^ns[a-z0-9]{3,}\.tmp$|^~nsu/i.test(name)), { appeared });
    step('the person’s own Musaeum folders are as they were', JSON.stringify(own.map(glance)) === JSON.stringify(ownBefore), { own });
    step('no Musaeum is left installed: no uninstall entry, no settings key, no Start menu shortcut', !registryHas(UNINSTALL_KEY) && registryHas(SETTINGS_KEY) === settingsBefore && existsSync(START_MENU) === startMenuBefore);
    report.finished = new Date().toISOString();
    mkdirSync(dirname(options.out), { recursive: true });
    writeFileSync(options.out, JSON.stringify(report, null, 2));
  }
  console.log(report.failures.length === 0 ? 'all passed' : `${report.failures.length} failed`);
  process.exitCode = report.failures.length === 0 ? 0 : 1;
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
