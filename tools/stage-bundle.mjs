#!/usr/bin/env node
// Gathers what the installer carries beside the app into target/bundle-stage, which
// apps/desktop/src-tauri/tauri.bundle.json points to, and refuses a stage that would ship broken:
//
// - the six workers and pdfium.dll from target/release, none of them asking for the Visual C++
//   runtime (the workspace links it statically, .cargo/config.toml), nor any other DLL Windows does
//   not have, besides pdfium.dll;
// - the por and eng tessdata_fast models with the SHA-256 the app pins (crates/musaeum-ocr/src/
//   models.rs), and their license;
// - LICENSE as LICENSE.txt, and THIRD-PARTY-NOTICES.md, which has to be current.
//
//   node tools/stage-bundle.mjs [--build target/release]
//
// `npm run build:bundle` runs this between building everything and `tauri build`.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { VC_RUNTIME, importsOf } from './lib/pe.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const at = process.argv.indexOf('--build');
const build = at > 0 ? resolve(process.argv[at + 1]) : join(root, 'target', 'release');
const stage = join(root, 'target', 'bundle-stage');

export const WORKERS = [
  'musaeum-render-pdf.exe',
  'musaeum-pdf-annots.exe',
  'musaeum-render-raster.exe',
  'musaeum-render-djvu.exe',
  'musaeum-ocr.exe',
  'musaeum-ingest.exe',
];
const SHIPPED_DLLS = new Set(['pdfium.dll']);
const MODELS = ['por', 'eng'];

const failures = [];
const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

/** The hashes the app checks its models against. */
function pinned() {
  const source = readFileSync(join(root, 'crates', 'musaeum-ocr', 'src', 'models.rs'), 'utf8');
  const found = new Map();
  for (const [, code, hash] of source.matchAll(/code: "([a-z_]+)",[\s\S]*?sha256: "([0-9a-f]{64})"/g)) {
    found.set(code, hash);
  }
  return found;
}

/** Whether a DLL is one Windows has on its own: in System32, and not the Visual C++ runtime. */
function fromWindows(name) {
  if (VC_RUNTIME.test(name)) {
    return false;
  }
  if (name.startsWith('api-ms-win-') || name.startsWith('ext-ms-win-')) {
    return true;
  }
  return existsSync(join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', name));
}

rmSync(stage, { recursive: true, force: true });
mkdirSync(join(stage, 'tessdata'), { recursive: true });

const binaries = ['musaeum-desktop.exe', ...WORKERS, 'pdfium.dll'];
for (const name of binaries) {
  const source = join(build, name);
  if (!existsSync(source)) {
    failures.push(`${name} is not in ${build}: build it first (npm run build:app)`);
    continue;
  }
  const imports = importsOf(source);
  const foreign = imports.filter((dll) => !SHIPPED_DLLS.has(dll) && !fromWindows(dll));
  if (foreign.length > 0) {
    failures.push(`${name} asks for DLLs Windows does not have: ${foreign.join(', ')}`);
  }
  if (name !== 'musaeum-desktop.exe') {
    copyFileSync(source, join(stage, name));
  }
}

const hashes = pinned();
for (const code of MODELS) {
  const source = join(build, 'tessdata', `${code}.traineddata`);
  if (!existsSync(source)) {
    failures.push(`tessdata/${code}.traineddata is not in ${build}`);
    continue;
  }
  if (sha256(source) !== hashes.get(code)) {
    failures.push(`tessdata/${code}.traineddata does not have the SHA-256 the app pins`);
    continue;
  }
  copyFileSync(source, join(stage, 'tessdata', `${code}.traineddata`));
}
const modelsLicense = join(build, 'tessdata', 'LICENSE');
if (existsSync(modelsLicense)) {
  copyFileSync(modelsLicense, join(stage, 'tessdata', 'LICENSE'));
} else {
  failures.push(`tessdata/LICENSE is not in ${build}`);
}

copyFileSync(join(root, 'LICENSE'), join(stage, 'LICENSE.txt'));
const notices = spawnSync(process.execPath, [join(root, 'tools', 'third-party-notices.mjs'), '--check'], { cwd: root, encoding: 'utf8' });
if (notices.status !== 0) {
  failures.push(notices.stderr.trim() || notices.stdout.trim());
} else {
  copyFileSync(join(root, 'THIRD-PARTY-NOTICES.md'), join(stage, 'THIRD-PARTY-NOTICES.md'));
}

if (failures.length > 0) {
  console.error('The bundle cannot be staged:');
  for (const failure of failures) {
    console.error(`  ${failure}`);
  }
  process.exit(1);
}
console.log(`Staged in ${stage}: ${WORKERS.length} workers, pdfium.dll, the ${MODELS.join(' and ')} models, LICENSE.txt and THIRD-PARTY-NOTICES.md. No binary asks for the Visual C++ runtime.`);
