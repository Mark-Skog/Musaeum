import { describe, expect, it } from 'vitest';

import type { BookRow, FullTextStatus, UnitHit } from '../api';
import { statusLine, targetOf } from './TextResults';
import { sameFilter } from './taxonomy-model';

const row: BookRow = {
  documentId: 7,
  workId: 3,
  title: 'Cosmos',
  creators: 'Carl Sagan',
  layout: 'flow',
  format: 'epub',
  instanceId: 1,
  revisionId: 1,
  folderId: 1,
  path: 'C:\\Livros\\cosmos.epub',
  state: 'present',
  blake3: 'ab',
  size: 10,
  progress: null,
  lastReadAt: null,
};

const hit = (kind: UnitHit['kind']): UnitHit => ({
  kind,
  index: 4,
  label: null,
  source: kind === 'page' ? 'text_layer' : 'flow',
  score: 1,
  snippet: '… um quasar distante …',
  marks: [[5, 11]],
  position: 120,
  matched: 'quasar',
});

describe('where a match opens its book', () => {
  it('a page match opens that page, at the match', () => {
    expect(targetOf(row, hit('page'))).toEqual({ kind: 'page', page: 4, position: 120 });
  });

  it('a flowing match opens the document and the place, with the length of what matched', () => {
    expect(targetOf(row, hit('document'))).toEqual({ kind: 'flow', document: 7, unit: 4, position: 120, length: 6 });
  });
});

describe('how far the index is', () => {
  const status = (pending: number, current: string | null = null): FullTextStatus => ({
    indexed: 3,
    empty: 1,
    failed: 1,
    pending,
    units: 40,
    chars: 9000,
    running: current !== null,
    current,
    read: 5,
  });

  it('says how many books are in it once nothing waits', () => {
    expect(statusLine(status(0))).toBe('Texto de 3 livros no índice.');
  });

  it('counts read, empty and failed books as done, and names the one being read', () => {
    expect(statusLine(status(5, 'Cosmos'))).toBe('Lendo o texto dos livros: 5 de 10. Lendo “Cosmos”.');
  });
});

describe('the review is a filter like the others', () => {
  it('is the same as itself and unlike any other', () => {
    expect(sameFilter({ kind: 'review' }, { kind: 'review' })).toBe(true);
    expect(sameFilter({ kind: 'review' }, { kind: 'all' })).toBe(false);
    expect(sameFilter({ kind: 'tag', id: 2 }, { kind: 'tag', id: 2 })).toBe(true);
    expect(sameFilter({ kind: 'tag', id: 2 }, { kind: 'tag', id: 3 })).toBe(false);
  });
});
