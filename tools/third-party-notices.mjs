#!/usr/bin/env node
// Writes THIRD-PARTY-NOTICES.md: everything in the app for Windows that others wrote, under its own
// license, with the license texts as their authors ask them to travel with the program.
//
//   node tools/third-party-notices.mjs            writes the file
//   node tools/third-party-notices.mjs --check    fails if the file is not what would be written
//
// What ships:
// - Rust crates: the normal dependencies (not the build or test ones) of the app and its workers on
//   x86_64-pc-windows-msvc, from Cargo.lock through `cargo metadata`, with the license files each
//   published crate carries.
// - npm packages: the ones package-lock.json does not mark as development only, which the page
//   bundles, with their license files.
// - What is vendored by hand (vendor/third-party.json): PDFium and what it contains, Tesseract,
//   Leptonica and the tessdata_fast models, DjVuLibre, libarchive, foliate-js and zip.js, and the
//   Noto fonts, embedded as subsets in the study reports.
//
// Each distinct license text appears once, with the components that carry it. The check needs the
// files that tools/fetch-pdfium.ps1, fetch-ocr.ps1 and fetch-fonts.ps1 download; without them it
// says so and passes, as the license check does.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = join(root, 'THIRD-PARTY-NOTICES.md');
const check = process.argv.includes('--check');
const TARGET = 'x86_64-pc-windows-msvc';
/** The packages whose executables the installer carries. */
const SHIPPED = ['musaeum-desktop', 'musaeum-render-pdf', 'musaeum-render-raster', 'musaeum-render-djvu', 'musaeum-ocr-worker', 'musaeum-library'];
const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense|copyright|authors)([-._].*)?$/i;

const normalize = (text) => text.replace(/\r\n?/g, '\n').replace(/[ \t]+$/gm, '').trim();
const digest = (text) => createHash('sha256').update(text).digest('hex').slice(0, 12);

/** The license files in a package's folder, by name. */
function licenseFiles(folder) {
  if (!existsSync(folder)) {
    return [];
  }
  return readdirSync(folder, { withFileTypes: true })
    .filter((entry) => entry.isFile() && LICENSE_FILE.test(entry.name))
    .map((entry) => entry.name)
    .sort()
    .map((name) => ({ name, text: normalize(readFileSync(join(folder, name), 'utf8')) }))
    .filter(({ text }) => text.length > 0);
}

function rustCrates() {
  const run = spawnSync('cargo', ['metadata', '--format-version', '1', '--locked', '--offline', '--filter-platform', TARGET], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 256 << 20,
  });
  if (run.status !== 0) {
    throw new Error(`cargo metadata failed:\n${run.stderr}`);
  }
  const metadata = JSON.parse(run.stdout);
  const members = new Set(metadata.workspace_members);
  const byId = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const start = metadata.packages.filter((pkg) => members.has(pkg.id) && SHIPPED.includes(pkg.name)).map((pkg) => pkg.id);
  if (start.length !== SHIPPED.length) {
    throw new Error(`not every shipped package is in the workspace: ${SHIPPED.join(', ')}`);
  }
  const seen = new Set();
  const queue = [...start];
  while (queue.length > 0) {
    const id = queue.pop();
    if (seen.has(id)) {
      continue;
    }
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      // Only what is linked into the program: no build scripts' or tests' dependencies.
      if (dep.dep_kinds.some((kind) => kind.kind === null)) {
        queue.push(dep.pkg);
      }
    }
  }
  return [...seen]
    .filter((id) => !members.has(id))
    .map((id) => byId.get(id))
    .map((pkg) => ({
      kind: 'Rust crate',
      name: pkg.name,
      version: pkg.version,
      license: pkg.license ?? (pkg.license_file ? `see ${pkg.license_file}` : 'not declared'),
      where: pkg.repository ?? `https://crates.io/crates/${pkg.name}`,
      files: licenseFiles(dirname(pkg.manifest_path)),
    }))
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

function npmPackages() {
  const desktop = join(root, 'apps', 'desktop');
  const lock = JSON.parse(readFileSync(join(desktop, 'package-lock.json'), 'utf8'));
  return Object.entries(lock.packages)
    .filter(([path, entry]) => path !== '' && !entry.dev)
    .map(([path, entry]) => {
      const name = path.replace(/^.*node_modules\//, '');
      return {
        kind: 'npm package',
        name,
        version: entry.version,
        license: entry.license,
        where: `https://www.npmjs.com/package/${name}`,
        files: licenseFiles(join(desktop, path)),
      };
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

function vendored() {
  const manifest = JSON.parse(readFileSync(join(root, 'vendor', 'third-party.json'), 'utf8'));
  let missing = 0;
  const components = manifest.components.map(({ name, license, file }) => {
    const path = join(root, file);
    if (!existsSync(path)) {
      missing++;
    }
    return {
      kind: 'vendored',
      name,
      version: '',
      license,
      where: file,
      files: existsSync(path) ? [{ name: file, text: normalize(readFileSync(path, 'utf8')) }] : [],
    };
  });
  return { components, missing };
}

function render(groups) {
  const all = groups.flatMap((group) => group.items);
  // Each text once, with who carries it.
  const texts = new Map();
  for (const item of all) {
    item.refs = [];
    for (const file of item.files) {
      const key = digest(file.text);
      if (!texts.has(key)) {
        texts.set(key, { key, text: file.text, users: [] });
      }
      texts.get(key).users.push(`${item.name}${item.version ? ` ${item.version}` : ''} (${file.name.split('/').at(-1)})`);
      item.refs.push(key);
    }
  }
  const lines = [];
  lines.push('# Third-party notices');
  lines.push('');
  lines.push(
    `Musaeum ${JSON.parse(readFileSync(join(root, 'apps', 'desktop', 'src-tauri', 'tauri.conf.json'), 'utf8')).version} is free software under the GNU General Public License, version 3 or later (\`LICENSE.txt\`). It contains the works listed here, each under its own license, compatible with the GPL; their notices and license texts follow, as their authors ask.`,
  );
  lines.push('');
  lines.push(
    'O Musaeum é software livre, sob a GPL versão 3 ou posterior. Ele contém as obras listadas aqui, cada uma sob a sua licença, compatível com a GPL; os avisos e os textos dessas licenças vêm a seguir, como os seus autores pedem.',
  );
  lines.push('');
  lines.push(
    'This file is written by `tools/third-party-notices.mjs` from `Cargo.lock` (the dependencies linked into the app and its workers on Windows x64), `apps/desktop/package-lock.json` (the packages the page bundles) and `vendor/third-party.json` (what is compiled from source or shipped as a binary). The Microsoft Edge WebView2 Runtime is part of Windows and is not distributed with Musaeum.',
  );
  lines.push('');
  for (const group of groups) {
    lines.push(`## ${group.title}`);
    lines.push('');
    lines.push(`${group.items.length} components.`);
    lines.push('');
    lines.push('| Component | Version | License | Source | License texts |');
    lines.push('|---|---|---|---|---|');
    for (const item of group.items) {
      const refs = item.refs.length > 0 ? item.refs.map((key) => `[${key}](#text-${key})`).join(', ') : 'none in the package: the standard text of its license applies';
      const cell = (value) => String(value).replaceAll('|', '\\|');
      lines.push(`| ${cell(item.name)} | ${cell(item.version)} | ${cell(item.license)} | ${cell(item.where)} | ${refs} |`);
    }
    lines.push('');
  }
  lines.push('## License texts');
  lines.push('');
  for (const { key, text, users } of [...texts.values()].sort((a, b) => a.key.localeCompare(b.key))) {
    lines.push(`### <a id="text-${key}"></a>${key}`);
    lines.push('');
    lines.push(`Carried by: ${users.join(', ')}.`);
    lines.push('');
    const fence = text.includes('```') ? '~~~~' : '```';
    lines.push(`${fence}text`);
    lines.push(text);
    lines.push(fence);
    lines.push('');
  }
  return lines.join('\n');
}

const { components, missing } = vendored();
if (missing > 0 && check) {
  console.log(`THIRD-PARTY-NOTICES.md not checked: ${missing} license files of vendored components are not fetched yet.`);
  process.exit(0);
}
const groups = [
  { title: 'Compiled from source or shipped as binaries', items: components },
  { title: 'Rust crates', items: rustCrates() },
  { title: 'npm packages bundled into the page', items: npmPackages() },
];
const text = `${render(groups)}\n`;
if (check) {
  const current = existsSync(output) ? readFileSync(output, 'utf8').replace(/\r\n/g, '\n') : '';
  if (current !== text) {
    console.error('THIRD-PARTY-NOTICES.md is out of date: run node tools/third-party-notices.mjs');
    process.exit(1);
  }
  console.log(`THIRD-PARTY-NOTICES.md is current: ${groups.map((group) => `${group.items.length} ${group.title.split(' ')[0].toLowerCase()}`).join(', ')}.`);
} else {
  writeFileSync(output, text);
  const without = groups.flatMap((group) => group.items).filter((item) => item.files.length === 0);
  console.log(`Wrote ${output}: ${groups.map((group) => `${group.title}: ${group.items.length}`).join('; ')}; ${(text.length / 1024).toFixed(0)} KiB.`);
  if (without.length > 0) {
    console.log(`Without a license file in the package: ${without.map((item) => `${item.name} ${item.version} (${item.license})`).join(', ')}`);
  }
}
