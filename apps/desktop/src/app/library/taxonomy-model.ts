/**
 * What the library's side and its editor work out without the host: the category tree and where
 * a category may move, a filter as the listing's arguments, the fields and operators a smart
 * collection's rule may use, and what an editor's draft changes in a work. The host checks every
 * change again; these checks only spare a person a round trip and an English message.
 */

import type { BatchEdit, BrowseFilter, Category, CustomField, CustomValue, FieldKind, Identifier, SmartRule, WorkEdit, WorkMetadata } from '../api';
import { t } from '../i18n';

/** What the side of the library has chosen. */
export type Filter =
  | { kind: 'all' }
  | { kind: 'folder'; id: number }
  | { kind: 'category'; id: number }
  | { kind: 'tag'; id: number }
  | { kind: 'collection'; id: number }
  | { kind: 'smart'; id: number }
  /** What waits for the person's word: documents and works that may be one. Not a listing. */
  | { kind: 'review' };

export function filterArgs(filter: Filter): BrowseFilter {
  return {
    folder: filter.kind === 'folder' ? filter.id : null,
    category: filter.kind === 'category' ? filter.id : null,
    tag: filter.kind === 'tag' ? filter.id : null,
    collection: filter.kind === 'collection' ? filter.id : null,
    smart: filter.kind === 'smart' ? filter.id : null,
  };
}

export function sameFilter(a: Filter, b: Filter): boolean {
  return a.kind === b.kind && (!('id' in a) || ('id' in b && a.id === b.id));
}

export interface CategoryNode {
  category: Category;
  children: CategoryNode[];
}

/** The categories as a tree, each level in the order the host lists them (the person's). */
export function categoryTree(categories: Category[]): CategoryNode[] {
  const nodes = new Map<number, CategoryNode>(categories.map((category) => [category.id, { category, children: [] }]));
  const roots: CategoryNode[] = [];
  for (const category of categories) {
    const node = nodes.get(category.id) as CategoryNode;
    const parent = category.parentId === null ? undefined : nodes.get(category.parentId);
    (parent?.children ?? roots).push(node);
  }
  return roots;
}

/** A category's name with its ancestors', `Ciências › Física`. */
export function categoryPath(categories: Category[], id: number): string {
  const byId = new Map(categories.map((category) => [category.id, category]));
  const names: string[] = [];
  for (let at = byId.get(id); at !== undefined; at = at.parentId === null ? undefined : byId.get(at.parentId)) {
    names.unshift(at.name);
    if (names.length > 64) {
      break;
    }
  }
  return names.join(' › ');
}

/** A category and every category under it. */
export function subtree(categories: Category[], id: number): Set<number> {
  const inside = new Set([id]);
  // The host lists parents before their children.
  for (const category of categories) {
    if (category.parentId !== null && inside.has(category.parentId)) {
      inside.add(category.id);
    }
  }
  return inside;
}

/** Where a category may go: the top, or any category not itself nor under it. */
export function moveTargets(categories: Category[], id: number): Category[] {
  const excluded = subtree(categories, id);
  return categories.filter((category) => !excluded.has(category.id));
}

/** The category before and after one among its siblings, for moving it up or down. */
export function siblingsAround(categories: Category[], id: number): { before: Category | null; after: Category | null } {
  const self = categories.find((category) => category.id === id);
  if (self === undefined) {
    return { before: null, after: null };
  }
  const siblings = categories.filter((category) => category.parentId === self.parentId);
  const at = siblings.findIndex((category) => category.id === id);
  return { before: siblings[at - 1] ?? null, after: siblings[at + 1] ?? null };
}

// --- Smart collections ---

export type RuleKind = 'text' | 'number' | 'moment' | 'day' | 'status' | 'format' | 'set' | 'boolean';

export interface RuleField {
  id: string;
  label: string;
  kind: RuleKind;
  /** For a set: what its value is chosen among. */
  set?: 'tag' | 'category' | 'collection' | 'folder';
  /** For a custom choice field: its choices. */
  choices?: string[];
}

/** The fields a rule may name: the library's own, then the person's custom fields. */
export function ruleFields(custom: CustomField[]): RuleField[] {
  const own: RuleField[] = [
    { id: 'title', label: t('taxonomy.field.title'), kind: 'text' },
    { id: 'creator', label: t('taxonomy.field.creator'), kind: 'text' },
    { id: 'series', label: t('taxonomy.field.series'), kind: 'text' },
    { id: 'language', label: t('taxonomy.field.language'), kind: 'text' },
    { id: 'identifier', label: t('taxonomy.field.identifier'), kind: 'text' },
    { id: 'year', label: t('taxonomy.field.year'), kind: 'number' },
    { id: 'rating', label: t('taxonomy.field.rating'), kind: 'number' },
    { id: 'progress', label: t('taxonomy.field.progress'), kind: 'number' },
    { id: 'status', label: t('taxonomy.field.status'), kind: 'status' },
    { id: 'format', label: t('taxonomy.field.format'), kind: 'format' },
    { id: 'added', label: t('taxonomy.field.added'), kind: 'moment' },
    { id: 'last_read', label: t('taxonomy.field.lastRead'), kind: 'moment' },
    { id: 'tag', label: t('taxonomy.field.tag'), kind: 'set', set: 'tag' },
    { id: 'category', label: t('taxonomy.field.category'), kind: 'set', set: 'category' },
    { id: 'collection', label: t('taxonomy.field.collection'), kind: 'set', set: 'collection' },
    { id: 'folder', label: t('taxonomy.field.folder'), kind: 'set', set: 'folder' },
  ];
  const kindOf: Record<FieldKind, RuleKind> = { text: 'text', url: 'text', choice: 'text', number: 'number', date: 'day', boolean: 'boolean' };
  return own.concat(
    custom.map((field) => ({
      id: `custom:${field.id}`,
      label: field.name,
      kind: kindOf[field.kind],
      choices: field.kind === 'choice' ? field.choices : undefined,
    })),
  );
}

export interface RuleOp {
  id: string;
  label: string;
  /** What the value is: none, one, two numbers. */
  takes: 'none' | 'one' | 'two';
}

/** Whether the field has a value at all: every kind but the ones always set. Words of the language shown. */
const empty = (): RuleOp[] => [
  { id: 'empty', label: t('taxonomy.op.empty'), takes: 'none' },
  { id: 'not_empty', label: t('taxonomy.op.notEmpty'), takes: 'none' },
];

/** The operators a kind of field takes, as the host's `smart.rs` accepts them. */
export function ruleOps(kind: RuleKind): RuleOp[] {
  switch (kind) {
    case 'text':
      return [
        { id: 'contains', label: t('taxonomy.op.contains'), takes: 'one' },
        { id: 'not_contains', label: t('taxonomy.op.notContains'), takes: 'one' },
        { id: 'is', label: t('taxonomy.op.is'), takes: 'one' },
        { id: 'is_not', label: t('taxonomy.op.isNot'), takes: 'one' },
        { id: 'starts_with', label: t('taxonomy.op.startsWith'), takes: 'one' },
        ...empty(),
      ];
    case 'number':
      return [
        { id: 'eq', label: '=', takes: 'one' },
        { id: 'ne', label: '≠', takes: 'one' },
        { id: 'lt', label: '<', takes: 'one' },
        { id: 'le', label: '≤', takes: 'one' },
        { id: 'gt', label: '>', takes: 'one' },
        { id: 'ge', label: '≥', takes: 'one' },
        { id: 'between', label: t('taxonomy.op.between'), takes: 'two' },
        ...empty(),
      ];
    case 'moment':
    case 'day':
      return [
        { id: 'in_last_days', label: t('taxonomy.op.inLastDays'), takes: 'one' },
        { id: 'before', label: t('taxonomy.op.before'), takes: 'one' },
        { id: 'after', label: t('taxonomy.op.after'), takes: 'one' },
        ...empty(),
      ];
    case 'status':
    case 'format':
      return [
        { id: 'is', label: t('taxonomy.op.is'), takes: 'one' },
        { id: 'is_not', label: t('taxonomy.op.isNot'), takes: 'one' },
      ];
    case 'set':
      return [
        { id: 'in', label: t('taxonomy.op.is'), takes: 'one' },
        { id: 'not_in', label: t('taxonomy.op.isNot'), takes: 'one' },
      ];
    case 'boolean':
      return [
        { id: 'is_true', label: t('taxonomy.op.yes'), takes: 'none' },
        { id: 'is_false', label: t('taxonomy.op.no'), takes: 'none' },
      ];
  }
}

/** The reading statuses, with their names in the language shown. */
export const statuses = (): [string, string][] => [
  ['unread', t('taxonomy.status.unread')],
  ['reading', t('taxonomy.status.reading')],
  ['read', t('taxonomy.status.read')],
  ['abandoned', t('taxonomy.status.abandoned')],
];

export const FORMATS = ['pdf', 'epub', 'mobi', 'azw3', 'fb2', 'djvu', 'cbz', 'cbr', 'txt', 'md'];

/** A rule for a field, with the first operator it takes and an empty value. */
export function defaultRule(field: RuleField): SmartRule {
  const op = ruleOps(field.kind)[0];
  const value = op.takes === 'none' ? null : field.kind === 'status' ? 'unread' : field.kind === 'format' ? 'pdf' : op.takes === 'two' ? [0, 0] : '';
  return { field: field.id, op: op.id, value };
}

/** Whether a date is written `YYYY-MM-DD` and is one. */
export function isDate(text: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(text)) {
    return false;
  }
  const [year, month, day] = text.split('-').map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
}

/** Why a rule cannot be kept yet, in the person's words; null if it can. */
export function ruleProblem(rule: SmartRule, fields: RuleField[]): string | null {
  const field = fields.find((candidate) => candidate.id === rule.field);
  if (field === undefined) {
    return t('taxonomy.problem.field');
  }
  const op = ruleOps(field.kind).find((candidate) => candidate.id === rule.op);
  if (op === undefined) {
    return t('taxonomy.problem.op');
  }
  if (op.takes === 'none') {
    return null;
  }
  const value = rule.value;
  if (op.takes === 'two') {
    return Array.isArray(value) && value.length === 2 && value.every((n) => typeof n === 'number' && Number.isFinite(n)) ? null : t('taxonomy.problem.twoNumbers');
  }
  if (field.kind === 'number' || op.id === 'in_last_days') {
    return typeof value === 'number' && Number.isFinite(value) ? null : t('taxonomy.problem.number');
  }
  if (field.kind === 'set') {
    return typeof value === 'number' ? null : t('taxonomy.problem.item');
  }
  if (op.id === 'before' || op.id === 'after') {
    return typeof value === 'string' && isDate(value) ? null : t('taxonomy.problem.date');
  }
  return typeof value === 'string' && value.trim() !== '' ? null : t('taxonomy.problem.value');
}

// --- The editor ---

/** An ISBN without spaces and hyphens, if its last digit checks; as `musaeum_library::isbn`. */
export function isbn(text: string): string | null {
  const digits = text.replace(/[-\s‐‑]/g, '').toUpperCase();
  if (/^\d{9}[\dX]$/.test(digits)) {
    let sum = 0;
    for (let at = 0; at < 10; at++) {
      sum += (digits[at] === 'X' ? 10 : Number(digits[at])) * (10 - at);
    }
    return sum % 11 === 0 ? digits : null;
  }
  if (/^\d{13}$/.test(digits)) {
    let sum = 0;
    for (let at = 0; at < 13; at++) {
      sum += Number(digits[at]) * (at % 2 === 0 ? 1 : 3);
    }
    return sum % 10 === 0 ? digits : null;
  }
  return null;
}

/** A language tag as BCP 47 writes it, or null; as `musaeum_library::language_tag`. */
export function languageTag(text: string): string | null {
  const parts = text.trim().split(/[-_]/);
  const [language, ...rest] = parts;
  if (!/^[A-Za-z]{2,3}$/.test(language ?? '')) {
    return null;
  }
  const out = [language.toLowerCase()];
  for (const part of rest) {
    if (!/^[A-Za-z0-9]{2,8}$/.test(part)) {
      return null;
    }
    if (/^[A-Za-z]{2}$/.test(part)) {
      out.push(part.toUpperCase());
    } else if (/^[A-Za-z]{4}$/.test(part)) {
      out.push(part[0].toUpperCase() + part.slice(1).toLowerCase());
    } else {
      out.push(part.toLowerCase());
    }
  }
  return out.join('-');
}

/** A work as the editor's fields hold it: text where a person types. */
export interface Draft {
  title: string;
  creators: { name: string; role: string }[];
  series: string;
  seriesPosition: string;
  year: string;
  language: string;
  identifiers: Identifier[];
  rating: number | null;
  status: string;
  description: string;
  categories: number[];
  tags: string[];
  collections: number[];
  custom: Record<number, string | number | boolean | null>;
}

export function draftOf(work: WorkMetadata): Draft {
  return {
    title: work.title,
    creators: work.creators.map((credit) => ({ ...credit })),
    series: work.series ?? '',
    seriesPosition: work.seriesPosition === null ? '' : String(work.seriesPosition),
    year: work.year === null ? '' : String(work.year),
    language: work.language ?? '',
    identifiers: work.identifiers.map((identifier) => ({ ...identifier })),
    rating: work.rating,
    status: work.status,
    description: work.description ?? '',
    categories: [...work.categories],
    tags: [...work.tagNames],
    collections: [...work.collections],
    custom: Object.fromEntries(work.custom.map((value) => [value.fieldId, value.value])),
  };
}

const sameList = <T>(a: T[], b: T[], same: (x: T, y: T) => boolean = (x, y) => x === y): boolean =>
  a.length === b.length && a.every((item, at) => same(item, b[at]));
const sameSet = (a: (number | string)[], b: (number | string)[]): boolean => a.length === b.length && a.every((item) => b.includes(item));

/** What the draft changes, and the problems that keep it from being saved, in the person's words. */
export function editOf(original: Draft, draft: Draft, fields: CustomField[]): { edit: WorkEdit; problems: string[] } {
  const edit: WorkEdit = {};
  const problems: string[] = [];
  const title = draft.title.trim();
  if (title !== original.title.trim()) {
    if (title === '') {
      problems.push(t('taxonomy.problem.title'));
    } else {
      edit.title = title;
    }
  }
  const creators = draft.creators.map((credit) => ({ name: credit.name.trim(), role: credit.role })).filter((credit) => credit.name !== '');
  if (!sameList(creators, original.creators, (a, b) => a.name === b.name && a.role === b.role)) {
    edit.creators = creators;
  }
  if (draft.series.trim() !== original.series.trim()) {
    edit.series = draft.series.trim() === '' ? null : draft.series.trim();
  }
  const number = (text: string): number | null | undefined => {
    const clean = text.trim().replace(',', '.');
    if (clean === '') {
      return null;
    }
    const value = Number(clean);
    return Number.isFinite(value) ? value : undefined;
  };
  if (draft.seriesPosition.trim() !== original.seriesPosition.trim()) {
    const value = number(draft.seriesPosition);
    if (value === undefined || (value !== null && (value < 0 || value > 1_000_000))) {
      problems.push(t('taxonomy.problem.seriesPosition'));
    } else {
      edit.seriesPosition = value;
    }
  }
  if (draft.year.trim() !== original.year.trim()) {
    const value = number(draft.year);
    if (value === undefined || (value !== null && (!Number.isInteger(value) || value < -9999 || value > 9999))) {
      problems.push(t('taxonomy.problem.year'));
    } else {
      edit.year = value;
    }
  }
  if (draft.language.trim() !== original.language.trim()) {
    if (draft.language.trim() === '') {
      edit.language = null;
    } else {
      const tag = languageTag(draft.language);
      if (tag === null) {
        problems.push(t('taxonomy.problem.language'));
      } else {
        edit.language = tag;
      }
    }
  }
  const identifiers = draft.identifiers
    .map((identifier) => ({ scheme: identifier.scheme, value: identifier.value.trim() }))
    .filter((identifier) => identifier.value !== '');
  if (!sameList(identifiers, original.identifiers, (a, b) => a.scheme === b.scheme && a.value === b.value)) {
    for (const identifier of identifiers) {
      if (identifier.scheme === 'isbn' && isbn(identifier.value) === null) {
        problems.push(t('taxonomy.problem.isbn', { value: identifier.value }));
      }
    }
    edit.identifiers = identifiers;
  }
  if (draft.rating !== original.rating) {
    edit.rating = draft.rating;
  }
  if (draft.status !== original.status) {
    edit.status = draft.status;
  }
  if (draft.description.trim() !== original.description.trim()) {
    edit.description = draft.description.trim() === '' ? null : draft.description.trim();
  }
  if (!sameSet(draft.categories, original.categories)) {
    edit.categories = draft.categories;
  }
  const tags = draft.tags.map((name) => name.trim()).filter((name) => name !== '');
  if (!sameSet(tags, original.tags)) {
    edit.tags = tags;
  }
  if (!sameSet(draft.collections, original.collections)) {
    edit.collections = draft.collections;
  }
  const custom: CustomValue[] = [];
  for (const field of fields) {
    const before = original.custom[field.id] ?? null;
    let after = draft.custom[field.id] ?? null;
    if (typeof after === 'string' && after.trim() === '') {
      after = null;
    }
    if (after === before || (typeof after === 'string' && typeof before === 'string' && after.trim() === before.trim())) {
      continue;
    }
    if (after !== null) {
      if (field.kind === 'number' && (typeof after !== 'number' || !Number.isFinite(after))) {
        const parsed = typeof after === 'string' ? Number(after.trim().replace(',', '.')) : NaN;
        if (!Number.isFinite(parsed)) {
          problems.push(t('taxonomy.problem.customNumber', { field: field.name }));
          continue;
        }
        after = parsed;
      }
      if (field.kind === 'date' && (typeof after !== 'string' || !isDate(after))) {
        problems.push(t('taxonomy.problem.customDate', { field: field.name }));
        continue;
      }
      if (field.kind === 'url' && (typeof after !== 'string' || !/^https?:\/\/\S+$/.test(after.trim()))) {
        problems.push(t('taxonomy.problem.customUrl', { field: field.name }));
        continue;
      }
    }
    custom.push({ fieldId: field.id, value: typeof after === 'string' ? after.trim() : after });
  }
  if (custom.length > 0) {
    edit.custom = custom;
  }
  return { edit, problems };
}

/** The roles a person has in a work, with their names in the language shown. */
export const roles = (): [string, string][] => [
  ['author', t('taxonomy.role.author')],
  ['editor', t('taxonomy.role.editor')],
  ['translator', t('taxonomy.role.translator')],
  ['illustrator', t('taxonomy.role.illustrator')],
  ['introduction', t('taxonomy.role.introduction')],
  ['contributor', t('taxonomy.role.contributor')],
];

/** The schemes of identifiers: standards' names, and one for any other. */
export const schemes = (): [string, string][] => [
  ['isbn', 'ISBN'],
  ['issn', 'ISSN'],
  ['doi', 'DOI'],
  ['oclc', 'OCLC'],
  ['lccn', 'LCCN'],
  ['asin', 'ASIN'],
  ['uri', 'URI'],
  ['other', t('taxonomy.scheme.otherScheme')],
];

/** The kinds of custom fields, with their names in the language shown. */
export const fieldKinds = (): [FieldKind, string][] => [
  ['text', t('taxonomy.kind.text')],
  ['number', t('taxonomy.kind.number')],
  ['date', t('taxonomy.kind.date')],
  ['boolean', t('taxonomy.kind.boolean')],
  ['choice', t('taxonomy.kind.choice')],
  ['url', t('taxonomy.kind.url')],
];

/** How a field's value came to be, in the person's words. */
export function provenanceLabel(origin: string | undefined): string | null {
  switch (origin) {
    case 'user':
      return t('taxonomy.origin.user');
    case 'embedded':
      return t('taxonomy.origin.embedded');
    case 'calibre':
      return t('taxonomy.origin.calibre');
    case 'filename':
      return t('taxonomy.origin.filename');
    default:
      return null;
  }
}

/** Letters of the Latin alphabets that carry their accent inside, not as a mark NFD takes apart. */
const UNMARKED: Record<string, string> = { ß: 'ss', đ: 'd', ø: 'o', ı: 'i' };

/** A name as the library compares names (ADR 0010, D2), like the core's `fold`: lower case, without
 * the accents of the Latin alphabets, one space between words. Marks on letters of other scripts stay. */
export function fold(text: string): string {
  return text
    .toLowerCase()
    .normalize('NFD')
    .replace(/(\p{Script=Latin})\p{M}+/gu, '$1')
    .normalize('NFC')
    .replace(/[ßđøı]/gu, (letter) => UNMARKED[letter] ?? letter)
    .replace(/\s+/gu, ' ')
    .trim();
}

/** The options with what was typed in their text, compared folded; all of them when nothing is. */
export function matching<T extends { text: string }>(options: readonly T[], typed: string): T[] {
  const wanted = fold(typed);
  return wanted === '' ? [...options] : options.filter((option) => fold(option.text).includes(wanted));
}

/** The option whose text is what was typed, compared folded, if there is one. */
export function named<T extends { text: string }>(options: readonly T[], typed: string): T | undefined {
  const wanted = fold(typed);
  return wanted === '' ? undefined : options.find((option) => fold(option.text) === wanted);
}

/**
 * What the panel changes on many books at once. A field is `undefined` until the person touches
 * it: only what was touched is sent, and set on every book; categories, tags and collections are
 * added to or taken from all of them.
 */
export interface BatchDraft {
  /** Names, separated by `;`. */
  creators?: string;
  series?: string;
  year?: string;
  language?: string;
  rating?: number | null;
  status?: string;
  addCategories: number[];
  removeCategories: number[];
  addTags: string[];
  removeTags: number[];
  addCollections: number[];
  removeCollections: number[];
}

export const emptyBatch = (): BatchDraft => ({
  addCategories: [],
  removeCategories: [],
  addTags: [],
  removeTags: [],
  addCollections: [],
  removeCollections: [],
});

/** What a batch draft changes, and the problems that keep it from being saved. */
export function batchEditOf(draft: BatchDraft): { edit: BatchEdit; problems: string[]; changed: boolean } {
  const edit: BatchEdit = {};
  const problems: string[] = [];
  if (draft.creators !== undefined) {
    const names = draft.creators
      .split(';')
      .map((name) => name.trim())
      .filter((name) => name !== '');
    edit.creators = names.map((name) => ({ name, role: 'author' }));
  }
  if (draft.series !== undefined) {
    edit.series = draft.series.trim() === '' ? null : draft.series.trim();
  }
  if (draft.year !== undefined) {
    const clean = draft.year.trim();
    if (clean === '') {
      edit.year = null;
    } else {
      const value = Number(clean);
      if (!Number.isInteger(value) || value < -9999 || value > 9999) {
        problems.push(t('taxonomy.problem.year'));
      } else {
        edit.year = value;
      }
    }
  }
  if (draft.language !== undefined) {
    if (draft.language.trim() === '') {
      edit.language = null;
    } else {
      const tag = languageTag(draft.language);
      if (tag === null) {
        problems.push(t('taxonomy.problem.language'));
      } else {
        edit.language = tag;
      }
    }
  }
  if (draft.rating !== undefined) {
    edit.rating = draft.rating;
  }
  if (draft.status !== undefined) {
    edit.status = draft.status;
  }
  const lists: [keyof BatchDraft & keyof BatchEdit, (number | string)[]][] = [
    ['addCategories', draft.addCategories],
    ['removeCategories', draft.removeCategories],
    ['addTags', draft.addTags],
    ['removeTags', draft.removeTags],
    ['addCollections', draft.addCollections],
    ['removeCollections', draft.removeCollections],
  ];
  for (const [key, values] of lists) {
    if (values.length > 0) {
      (edit as Record<string, unknown>)[key] = [...values];
    }
  }
  return { edit, problems, changed: Object.keys(edit).length > 0 };
}

