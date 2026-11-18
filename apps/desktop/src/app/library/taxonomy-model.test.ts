import { describe, expect, it } from 'vitest';

import type { Category, CustomField, WorkMetadata } from '../api';
import {
  categoryPath,
  categoryTree,
  defaultRule,
  draftOf,
  editOf,
  filterArgs,
  fold,
  isbn,
  isDate,
  languageTag,
  matching,
  moveTargets,
  named,
  ruleFields,
  ruleOps,
  ruleProblem,
  siblingsAround,
} from './taxonomy-model';

const category = (id: number, parentId: number | null, name: string, depth: number): Category => ({ id, parentId, name, depth, works: 0, total: 0 });

// Depth first, as the host lists them.
const TREE: Category[] = [
  category(1, null, 'Ciências', 0),
  category(2, 1, 'Biologia', 1),
  category(3, 1, 'Física', 1),
  category(4, 3, 'Quântica', 2),
  category(5, null, 'Literatura', 0),
];

describe('the category tree', () => {
  it('nests each category under its parent, in the listed order', () => {
    const tree = categoryTree(TREE);
    expect(tree.map((node) => node.category.name)).toEqual(['Ciências', 'Literatura']);
    expect(tree[0].children.map((node) => node.category.name)).toEqual(['Biologia', 'Física']);
    expect(tree[0].children[1].children[0].category.name).toBe('Quântica');
    expect(categoryPath(TREE, 4)).toBe('Ciências › Física › Quântica');
  });

  it('offers no place under a category itself or under its own to move it to', () => {
    expect(moveTargets(TREE, 3).map((target) => target.id)).toEqual([1, 2, 5]);
    expect(moveTargets(TREE, 1).map((target) => target.id)).toEqual([5]);
    expect(siblingsAround(TREE, 3)).toEqual({ before: TREE[1], after: null });
    expect(siblingsAround(TREE, 1).after?.id).toBe(5);
  });

  it('turns what the side chose into the listing’s arguments', () => {
    expect(filterArgs({ kind: 'category', id: 3 })).toEqual({ folder: null, category: 3, tag: null, collection: null, smart: null });
    expect(filterArgs({ kind: 'all' })).toEqual({ folder: null, category: null, tag: null, collection: null, smart: null });
  });
});

describe('a smart collection’s rule', () => {
  const custom: CustomField[] = [{ id: 7, name: 'Situação', kind: 'choice', choices: ['em casa', 'emprestado'] }];
  const fields = ruleFields(custom);

  it('offers each field the operators the host accepts for it', () => {
    expect(ruleOps('number').map((op) => op.id)).toContain('between');
    expect(ruleOps('set').map((op) => op.id)).toEqual(['in', 'not_in']);
    const situation = fields.find((field) => field.id === 'custom:7');
    expect(situation?.kind).toBe('text');
    expect(situation?.choices).toEqual(['em casa', 'emprestado']);
  });

  it('says what is missing before it can be kept', () => {
    const year = fields.find((field) => field.id === 'year');
    expect(year).toBeDefined();
    const rule = defaultRule(year!);
    expect(ruleProblem(rule, fields)).toBe('Informe um número.');
    expect(ruleProblem({ ...rule, value: 1990 }, fields)).toBeNull();
    expect(ruleProblem({ field: 'year', op: 'between', value: [1980, 1990] }, fields)).toBeNull();
    expect(ruleProblem({ field: 'added', op: 'after', value: '2024-02-30' }, fields)).toMatch(/data/);
    expect(ruleProblem({ field: 'title', op: 'empty', value: null }, fields)).toBeNull();
    expect(ruleProblem({ field: 'nothing', op: 'is', value: 'x' }, fields)).toBe('Escolha um campo.');
  });
});

describe('the editor', () => {
  const work: WorkMetadata = {
    id: 9,
    title: 'Vida Maravilhosa',
    creators: [{ name: 'Stephen Jay Gould', role: 'author' }],
    series: null,
    seriesPosition: null,
    year: 1990,
    language: 'pt-BR',
    identifiers: [{ scheme: 'isbn', value: '9788535902778' }],
    rating: null,
    status: 'unread',
    categories: [1],
    tags: [3],
    tagNames: ['Clássico'],
    collections: [],
    custom: [{ fieldId: 7, value: 'em casa' }],
    provenance: { title: 'embedded' },
    documents: [9],
    formats: ['pdf'],
  };
  const fields: CustomField[] = [
    { id: 7, name: 'Situação', kind: 'choice', choices: ['em casa', 'emprestado'] },
    { id: 8, name: 'Páginas', kind: 'number', choices: [] },
  ];
  const original = draftOf(work);

  it('changes nothing when nothing was touched', () => {
    expect(editOf(original, structuredClone(original), fields)).toEqual({ edit: {}, problems: [] });
  });

  it('sends only what changed, cleared fields as nothing', () => {
    const draft = structuredClone(original);
    draft.title = '  Vida Maravilhosa (2ª ed.) ';
    draft.year = '';
    draft.language = 'en-us';
    draft.creators.push({ name: 'Paulo Geiger', role: 'translator' });
    draft.tags = ['Clássico', 'Biologia'];
    draft.custom[8] = '402';
    draft.custom[7] = '';
    const { edit, problems } = editOf(original, draft, fields);
    expect(problems).toEqual([]);
    expect(edit).toEqual({
      title: 'Vida Maravilhosa (2ª ed.)',
      year: null,
      language: 'en-US',
      creators: [
        { name: 'Stephen Jay Gould', role: 'author' },
        { name: 'Paulo Geiger', role: 'translator' },
      ],
      tags: ['Clássico', 'Biologia'],
      custom: [
        { fieldId: 7, value: null },
        { fieldId: 8, value: 402 },
      ],
    });
  });

  it('says in the person’s words what cannot be saved', () => {
    const draft = structuredClone(original);
    draft.title = ' ';
    draft.year = '19x0';
    draft.language = 'português';
    draft.identifiers = [{ scheme: 'isbn', value: '978-85-359-0277-9' }];
    const { problems } = editOf(original, draft, fields);
    expect(problems).toHaveLength(4);
  });

  it('checks an ISBN, a language and a date as the host does', () => {
    expect(isbn('978-85-359-0277-8')).toBe('9788535902778');
    expect(isbn('0 8044 2957 x')).toBe('080442957X');
    expect(isbn('978-85-359-0277-9')).toBeNull();
    expect(languageTag('zh_hant_tw')).toBe('zh-Hant-TW');
    expect(languageTag('p')).toBeNull();
    expect(isDate('2024-02-29')).toBe(true);
    expect(isDate('2023-02-29')).toBe(false);
  });
});

describe('the picker of the details', () => {
  const options = [
    { key: '1', text: 'Ciências' },
    { key: '2', text: 'Ciências › Biologia' },
    { key: '4', text: 'Literatura' },
    { key: '3', text: 'Literatura › Física Moderna' },
  ];

  it('compares names as the core does', () => {
    expect(fold('  Física   Moderna ')).toBe('fisica moderna');
    expect(fold('ÇÃO Straße Øre')).toBe('cao strasse ore');
    // Only the Latin alphabets lose their accents: a Devanagari vowel sign is part of the word.
    expect(fold('हिंदी')).toBe('हिंदी'.normalize('NFC'));
  });

  it('lists what has what was typed in it, without case or accents', () => {
    expect(matching(options, 'Biolo').map((option) => option.key)).toEqual(['2']);
    expect(matching(options, 'ciencias').map((option) => option.key)).toEqual(['1', '2']);
    expect(matching(options, 'fisica').map((option) => option.key)).toEqual(['3']);
    expect(matching(options, '  ')).toHaveLength(4);
    expect(matching(options, 'zoologia')).toEqual([]);
  });

  it('knows an option named exactly, so Enter takes it instead of making another', () => {
    expect(named(options, 'literatura')?.key).toBe('4');
    expect(named(options, 'Litera')).toBeUndefined();
    expect(named(options, '')).toBeUndefined();
  });
});
