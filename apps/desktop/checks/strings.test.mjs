/**
 * No word of the interface is written outside the catalogs: every text of a component, every label
 * a person or a screen reader gets, every message built for a person, comes from `t()`.
 *
 * The sources are read with the parser Vite already has (Rolldown's), and each string that reads
 * like words is a finding unless it is code: a class, a selector, an event's name, a key of the
 * catalogs, an internal error a person never reads, or one of the few names below that are the
 * same in every language.
 */

import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseAst } from 'rolldown/parseAst';
import { describe, expect, it } from 'vitest';

const ROOT = fileURLToPath(new URL('../src', import.meta.url));

/** The interface: the app's components and what builds their words, and the reader's chrome. */
const SCANNED = ['app', 'reader/continuous-view.ts'];
const SKIPPED_FILES = [/\.test\.tsx?$/, /\.d\.ts$/, /^app\/i18n\//];

/** Attributes whose value is code, not words. */
const CODE_ATTRIBUTES = new Set([
  'className', 'id', 'key', 'type', 'role', 'href', 'src', 'slot', 'name', 'value', 'htmlFor', 'autoComplete', 'inputMode', 'lang', 'dir',
  'target', 'rel', 'form', 'orientation', 'selectionMode', 'selectionBehavior', 'layout', 'placement', 'variant', 'density', 'level', 'accept',
  'pattern', 'd', 'viewBox', 'fill', 'stroke', 'xmlns', 'width', 'height', 'tabIndex', 'menuTrigger', 'escapeKeyBehavior', 'disabledBehavior',
  'keyboardNavigationBehavior', 'trigger', 'axis', 'direction', 'stretch', 'items', 'defaultSelectedKeys', 'selectedKey', 'defaultValue',
  'data-field', 'data-picker', 'data-document', 'data-surface', 'data-action', 'data-kind', 'data-state', 'data-busy', 'data-tool', 'data-color',
  'aria-hidden', 'aria-live', 'aria-atomic', 'aria-busy', 'aria-current', 'aria-haspopup', 'aria-controls', 'aria-expanded', 'aria-modal',
  'aria-orientation', 'aria-pressed', 'aria-selected', 'aria-level', 'aria-sort', 'aria-relevant', 'aria-owns', 'aria-describedby',
  'aria-labelledby', 'aria-errormessage', 'aria-invalid', 'aria-disabled', 'aria-readonly', 'aria-required', 'aria-multiselectable',
  'style', 'shortcut', 'format', 'kind', 'surface', 'tool', 'color', 'mode', 'size', 'theme', 'motion', 'step', 'min', 'max',
  'loading', 'decoding', 'aria-keyshortcuts', 'scope', 'headers', 'colSpan', 'rowSpan', 'field',
]);

/** Calls whose string arguments are code. */
const CODE_CALLS = new Set([
  'querySelector', 'querySelectorAll', 'closest', 'matches', 'getElementById', 'getAttribute', 'setAttribute', 'removeAttribute', 'hasAttribute',
  'add', 'remove', 'toggle', 'contains', 'addEventListener', 'removeEventListener', 'invoke', 'listen', 'emit', 'matchMedia', 'getPropertyValue',
  'setProperty', 'createElement', 'startsWith', 'endsWith', 'includes', 'split', 'replace', 'replaceAll', 'join', 'padStart', 'indexOf',
  't', 'tn', 'tIn', 'log', 'warn', 'error', 'info', 'debug', 'Error', 'TypeError', 'RangeError', 'reject', 'postMessage', 'send', 'get', 'set',
  'has', 'delete', 'localeCompare', 'toLocaleUpperCase', 'NumberFormat', 'DateTimeFormat', 'DisplayNames', 'RegExp', 'fetch', 'URL', 'Blob',
  'dispatchEvent', 'CustomEvent', 'Event', 'KeyboardEvent', 'focus', 'scrollIntoView', 'animate', 'evaluate', 'stringify', 'parse', 'template',
]);

/** Names that are the same in every language: products, formats, standards, symbols. */
const SAME_IN_EVERY_LANGUAGE = new Set([
  'Musaeum', 'PDF', 'EPUB', 'XFDF', 'JSON-LD', 'OCR', 'ISBN', 'URI', 'URL', 'DjVu', 'CBZ', 'MOBI', 'AZW3', 'FB2', 'TXT', 'MD', 'Calibre',
  'Ctrl+,', 'Ctrl+A', 'Esc', 'Tab', 'Shift', 'Enter', 'Home', 'End', 'PageUp', 'PageDown', 'Português (Brasil)', 'English', 'Markdown', 'W3C',
  'BCP 47', 'ASIN', 'DOI', 'OCLC', 'LCCN', 'Google', 'Amazon', 'Goodreads', 'SHA-256', 'UUID', 'UTF-8',
  'pt-BR', 'en', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Escape', 'A−', 'A+', 'Aa',
]);

const letters = /\p{L}/u;
const accented = /[À-ÖØ-öø-ÿ]/;
/** A line that says so is not for people: a test's hook, a message only a test reads. */
const NOT_FOR_PEOPLE = 'i18n: not for people';

/** Whether a string, out of any attribute, reads like words rather than code. */
function wordy(text) {
  const trimmed = text.trim();
  if (!letters.test(trimmed) || SAME_IN_EVERY_LANGUAGE.has(trimmed)) {
    return false;
  }
  if (accented.test(trimmed)) {
    return true;
  }
  // A key of the catalogs, an event, a class or an identifier: no spaces, lowercase, dots or dashes.
  if (/^[a-z0-9][\w.:/#@-]*$/.test(trimmed)) {
    return false;
  }
  // A list of element names, for a selector.
  if (/^[a-z][a-z0-9]*(, [a-z][a-z0-9]*)+$/.test(trimmed)) {
    return false;
  }
  // A selector, a CSS value or a path.
  if (/^[.#[:(@*>~+]|^(?:var|url|rgb|rgba|calc|translate)\(|^[\w-]+:\s|[{};]|^[\w-]+\s*[>~+]\s*[\w-]|^\[|\/\//.test(trimmed)) {
    return false;
  }
  // Class lists: lowercase words with dashes, separated by spaces.
  if (/^[a-z][a-z0-9-]*( [a-z][a-z0-9-]*)+$/.test(trimmed) && !/ (de|do|da|o|a|e|em|no|na|um|uma|the|of|to|and|in|is|a|an) /.test(` ${trimmed} `)) {
    return false;
  }
  return /\s/.test(trimmed) || /^\p{Lu}\p{Ll}+/u.test(trimmed) || /…/.test(trimmed);
}

function scan(file) {
  const source = readFileSync(file, 'utf8');
  const ast = parseAst(source, { lang: file.endsWith('.tsx') ? 'tsx' : 'ts' });
  const rel = relative(ROOT, file).replaceAll('\\', '/');
  const lineOf = (at) => source.slice(0, at).split('\n').length;
  const found = [];
  const calleeName = (node) => {
    const callee = node.callee;
    if (callee?.type === 'Identifier') {
      return callee.name;
    }
    if (callee?.type === 'MemberExpression' || callee?.type === 'StaticMemberExpression') {
      return callee.property?.name ?? null;
    }
    return null;
  };
  const walk = (node, context) => {
    if (node === null || typeof node !== 'object') {
      return;
    }
    if (Array.isArray(node)) {
      for (const child of node) {
        walk(child, context);
      }
      return;
    }
    const current = node;
    let next = context;
    switch (current.type) {
      case 'JSXText': {
        const text = String(current.value).trim();
        if (letters.test(text) && !SAME_IN_EVERY_LANGUAGE.has(text)) {
          found.push({ file: rel, line: lineOf(current.start), kind: 'jsx-text', text });
        }
        return;
      }
      case 'JSXAttribute': {
        const name = current.name;
        const attribute = typeof name?.name === 'string' ? name.name : 'namespaced';
        const value = current.value;
        if (value?.type === 'Literal' && typeof value.value === 'string') {
          const text = value.value;
          if (!CODE_ATTRIBUTES.has(attribute) && !attribute.startsWith('data-') && letters.test(text) && !SAME_IN_EVERY_LANGUAGE.has(text)) {
            found.push({ file: rel, line: lineOf(current.start), kind: `attribute ${attribute}`, text });
          }
          return;
        }
        next = { ...context, attribute };
        break;
      }
      case 'CallExpression':
      case 'NewExpression': {
        const name = calleeName(current);
        if (name !== null && CODE_CALLS.has(name)) {
          walk(current.callee, context);
          walk(current.arguments, { ...context, codeCall: true });
          return;
        }
        break;
      }
      case 'ImportDeclaration':
      case 'ExportAllDeclaration':
      case 'ImportExpression':
      case 'TSLiteralType':
      case 'TSTypeAnnotation':
      case 'TSTypeAliasDeclaration':
      case 'TSInterfaceDeclaration':
      case 'TSAsExpression':
      case 'TSSatisfiesExpression':
        if (current.type === 'TSAsExpression' || current.type === 'TSSatisfiesExpression') {
          walk(current.expression, context);
        }
        return;
      case 'Property':
      case 'ObjectProperty': {
        walk(current.value, context);
        return;
      }
      case 'SwitchCase': {
        walk(current.consequent, context);
        return;
      }
      case 'BinaryExpression': {
        if (['===', '!==', '==', '!='].includes(current.operator)) {
          return;
        }
        break;
      }
      case 'Literal': {
        if (typeof current.value === 'string' && !context.codeCall && (context.attribute === null || !CODE_ATTRIBUTES.has(context.attribute)) && wordy(current.value)) {
          found.push({ file: rel, line: lineOf(current.start), kind: context.attribute === null ? 'string' : `in ${context.attribute}`, text: current.value });
        }
        return;
      }
      case 'TemplateLiteral': {
        const text = current.quasis.map((quasi) => quasi.value.cooked ?? quasi.value.raw).join('{…}');
        if (!context.codeCall && (context.attribute === null || !CODE_ATTRIBUTES.has(context.attribute)) && wordy(text.replaceAll('{…}', ' '))) {
          found.push({ file: rel, line: lineOf(current.start), kind: 'template', text });
        }
        walk(current.expressions, context);
        return;
      }
      default:
        break;
    }
    for (const [key, value] of Object.entries(current)) {
      if (key !== 'parent' && value !== null && typeof value === 'object') {
        walk(value, next);
      }
    }
  };
  walk(ast, { attribute: null, codeCall: false, typePosition: false, key: false });
  const marked = new Set(source.split('\n').flatMap((line, index) => (line.includes(NOT_FOR_PEOPLE) ? [index + 1, index + 2] : [])));
  return found.filter((finding) => !marked.has(finding.line));
}

function files() {
  const out = [];
  for (const entry of SCANNED) {
    const path = join(ROOT, entry);
    const visit = (at) => {
      for (const item of readdirSync(at, { withFileTypes: true })) {
        const child = join(at, item.name);
        if (item.isDirectory()) {
          visit(child);
        } else if (/\.tsx?$/.test(item.name)) {
          out.push(child);
        }
      }
    };
    if (entry.endsWith('.ts')) {
      out.push(path);
    } else {
      visit(path);
    }
  }
  return out.filter((file) => !SKIPPED_FILES.some((pattern) => pattern.test(relative(ROOT, file).replaceAll('\\', '/'))));
}

describe('the interface', () => {
  it('writes no word outside the catalogs', () => {
    const findings = files().flatMap(scan);
    const shown = findings.map((finding) => `${finding.file}:${finding.line} [${finding.kind}] ${finding.text.slice(0, 100)}`);
    if (process.env.STRINGS_LIST !== undefined) {
      // For whoever is moving words into the catalogs: the whole list, one per line.
      console.log(shown.join('\n'));
    }
    expect(shown).toEqual([]);
  });
});

/** Every string and every template's text before its first value, in the app's own code. */
function literalsOf(file) {
  const source = readFileSync(file, 'utf8');
  const ast = parseAst(source, { lang: file.endsWith('.tsx') ? 'tsx' : 'ts' });
  const strings = new Set();
  const prefixes = new Set();
  const walk = (node) => {
    if (node === null || typeof node !== 'object') {
      return;
    }
    if (Array.isArray(node)) {
      node.forEach(walk);
      return;
    }
    if (node.type === 'Literal' && typeof node.value === 'string') {
      strings.add(node.value);
    }
    if (node.type === 'TemplateLiteral') {
      const head = node.quasis[0]?.value.cooked ?? '';
      if (node.quasis.length === 1) {
        strings.add(head);
      } else if (head.length > 0) {
        prefixes.add(head);
      }
    }
    for (const [key, value] of Object.entries(node)) {
      if (key !== 'parent' && value !== null && typeof value === 'object') {
        walk(value);
      }
    }
  };
  walk(ast);
  return { strings, prefixes };
}

describe('the catalogs', () => {
  it('have no key that nothing uses', () => {
    const catalogs = join(ROOT, 'app/i18n/pt-BR');
    const keys = readdirSync(catalogs).flatMap((name) =>
      [...readFileSync(join(catalogs, name), 'utf8').matchAll(/^\s+'([a-z][\w.]*)':/gm)].map((match) => match[1]),
    );
    expect(keys.length).toBeGreaterThan(300);
    const strings = new Set();
    const prefixes = new Set();
    const visit = (at) => {
      for (const item of readdirSync(at, { withFileTypes: true })) {
        const child = join(at, item.name);
        const rel = relative(ROOT, child).replaceAll('\\', '/');
        if (item.isDirectory()) {
          visit(child);
        } else if (/\.tsx?$/.test(item.name) && !SKIPPED_FILES.some((pattern) => pattern.test(rel))) {
          const found = literalsOf(child);
          found.strings.forEach((value) => strings.add(value));
          found.prefixes.forEach((value) => prefixes.add(value));
        }
      }
    };
    visit(ROOT);
    const used = (key) =>
      strings.has(key) ||
      strings.has(key.replace(/\.(one|other)$/, '')) ||
      [...prefixes].some((prefix) => prefix.includes('.') && key.startsWith(prefix));
    expect(keys.filter((key) => !used(key))).toEqual([]);
  });
});

/** Buttons whose face is only a symbol or a picture, and that have no name of their own. */
function namelessButtons(file) {
  const source = readFileSync(file, 'utf8');
  const ast = parseAst(source, { lang: file.endsWith('.tsx') ? 'tsx' : 'ts' });
  const rel = relative(ROOT, file).replaceAll('\\', '/');
  const lineOf = (at) => source.slice(0, at).split('\n').length;
  const found = [];
  const BUTTONS = new Set(['Button', 'ToggleButton', 'button']);
  const named = (opening) =>
    opening.attributes.some((attribute) => ['aria-label', 'aria-labelledby'].includes(attribute.name?.name) || attribute.type === 'JSXSpreadAttribute');
  /** Whether a child shows words: text with letters, or a value put in (a `t()` or a variable). */
  const wordy = (child) => {
    if (child.type === 'JSXText') {
      return /\p{L}/u.test(child.value);
    }
    if (child.type === 'JSXExpressionContainer') {
      return child.expression.type !== 'JSXEmptyExpression';
    }
    if (child.type === 'JSXElement') {
      // React Aria names a select's button by its label and the value it shows.
      if (child.openingElement.name?.name === 'SelectValue') {
        return true;
      }
      const hidden = child.openingElement.attributes.some((attribute) => attribute.name?.name === 'aria-hidden');
      return !hidden && child.children.some(wordy);
    }
    return false;
  };
  const walk = (node) => {
    if (node === null || typeof node !== 'object') {
      return;
    }
    if (Array.isArray(node)) {
      node.forEach(walk);
      return;
    }
    if (node.type === 'JSXElement') {
      const name = node.openingElement.name?.name;
      if (BUTTONS.has(name) && !named(node.openingElement) && !node.children.some(wordy)) {
        found.push(`${rel}:${lineOf(node.start)}`);
      }
    }
    for (const [key, value] of Object.entries(node)) {
      if (key !== 'parent' && value !== null && typeof value === 'object') {
        walk(value);
      }
    }
  };
  walk(ast);
  return found;
}

describe('the buttons', () => {
  it('that show only a symbol have a name a screen reader says', () => {
    expect(files().filter((file) => file.endsWith('.tsx')).flatMap(namelessButtons)).toEqual([]);
  });
});
