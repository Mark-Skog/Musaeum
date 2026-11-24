#!/usr/bin/env node
// Checks every dependency against the list of licenses compatible with GPL-3.0-or-later: Rust
// crates through cargo-deny, npm packages through the lockfile, and whatever is vendored by hand
// through vendor/third-party.json. Fails on anything else, so a new license needs a decision.
//
//   node tools/check-licenses.mjs
//
// The pre-commit hook in .githooks runs this when a dependency changes, and so does CI.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { isSatisfiable } from './lib/spdx.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** The `allow` list of deny.toml, so that there is one list to maintain. */
function allowedByDeny() {
  const text = readFileSync(join(root, 'deny.toml'), 'utf8');
  const block = text.match(/^allow = \[([\s\S]*?)^\]/m);
  if (block === null) {
    throw new Error('deny.toml has no allow list');
  }
  return [...block[1].matchAll(/^\s*"([^"]+)"/gm)].map((match) => match[1]);
}

const allowed = new Set(allowedByDeny());
// Only in what is vendored, and no crate or package has them. All are permissive and compatible
// with GPL-3.0, though FreeType's is not compatible with GPL-2.0-only, and asks for a credit. The
// one copyleft license is DjVuLibre's, which the plan approved: its 'or later' lets the app use it
// under GPL-3.0, the app's own license.
const allowedForVendored = new Set([
  ...allowed,
  'AGG-2.3',
  'FTL',
  'GPL-2.0-or-later',
  'ICU',
  'IJG',
  'libpng-2.0',
  // Fonts, embedded as subsets in the reports the app writes. The OFL lets them be bundled with any
  // software and embedded in documents; only the fonts themselves stay under it.
  'OFL-1.1',
]);

const failures = [];

function checkRust() {
  const run = spawnSync('cargo', ['deny', '--color', 'never', 'check', 'licenses', 'bans', 'sources'], {
    cwd: root,
    encoding: 'utf8',
  });
  if (run.error !== undefined) {
    failures.push(`cargo-deny could not run (cargo install cargo-deny): ${run.error.message}`);
    return 'not checked';
  }
  if (run.status !== 0) {
    failures.push(`cargo-deny refused the Rust dependencies:\n${run.stderr}`);
    return 'refused';
  }
  return `${run.stdout}\n${run.stderr}`.trim().split('\n').at(-1);
}

function checkNpm() {
  const lock = JSON.parse(readFileSync(join(root, 'apps', 'desktop', 'package-lock.json'), 'utf8'));
  const counts = { shipped: 0, development: 0 };
  for (const [path, entry] of Object.entries(lock.packages)) {
    if (path === '') {
      continue;
    }
    counts[entry.dev ? 'development' : 'shipped']++;
    if (typeof entry.license !== 'string') {
      failures.push(`${path} does not declare a license`);
    } else if (!isSatisfiable(entry.license, allowed)) {
      failures.push(`${path} is under ${entry.license}, which is not on the list`);
    }
  }
  return `${counts.shipped} shipped, ${counts.development} development only`;
}

function checkVendored() {
  const manifest = JSON.parse(readFileSync(join(root, 'vendor', 'third-party.json'), 'utf8'));
  let notFetched = 0;
  for (const { name, license, file, fetched } of manifest.components) {
    if (!isSatisfiable(license, allowedForVendored)) {
      failures.push(`${name} is under ${license}, which is not on the list`);
    }
    if (!existsSync(join(root, file))) {
      if (fetched) {
        notFetched++;
      } else {
        failures.push(`${name} has no license file at ${file}`);
      }
    }
  }
  // Nothing vendored may be missing from the manifest.
  const pinned = readFileSync(join(root, 'tools', 'fetch-pdfium.ps1'), 'utf8').match(/\$release = '(\d+)'/);
  if (pinned !== null && !manifest.components.some(({ file }) => file.includes(`chromium-${pinned[1]}`))) {
    failures.push(`vendor/third-party.json does not cover PDFium chromium/${pinned[1]}`);
  }
  // Nor what tools/fetch-ocr.ps1 pins: each source folder it unpacks has its entry.
  const ocr = readFileSync(join(root, 'tools', 'fetch-ocr.ps1'), 'utf8');
  for (const [, folder] of ocr.matchAll(/Name = '([^']+)'/g)) {
    if (!manifest.components.some(({ file }) => file.startsWith(`vendor/tesseract/${folder}/`))) {
      failures.push(`vendor/third-party.json does not cover ${folder}`);
    }
  }
  // Nor the fonts tools/fetch-fonts.ps1 pins: they are covered by the license file it fetches.
  const fonts = readFileSync(join(root, 'tools', 'fetch-fonts.ps1'), 'utf8');
  if (fonts.includes("'OFL.txt'") && !manifest.components.some(({ file }) => file === 'vendor/fonts/OFL.txt')) {
    failures.push('vendor/third-party.json does not cover the fonts of tools/fetch-fonts.ps1');
  }
  const fetchedNote = notFetched > 0 ? `, ${notFetched} license files not fetched yet` : '';
  return `${manifest.components.length} components${fetchedNote}`;
}

console.log(`Rust: ${checkRust()}`);
console.log(`npm: ${checkNpm()}`);
console.log(`Vendored: ${checkVendored()}`);

if (failures.length > 0) {
  console.error('\nLicense check failed:');
  for (const failure of failures) {
    console.error(`  ${failure}`);
  }
  process.exit(1);
}
console.log('All licenses are on the list.');
