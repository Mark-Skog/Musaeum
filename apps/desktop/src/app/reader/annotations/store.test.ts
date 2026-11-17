import { describe, expect, it } from 'vitest';
import type { Annotation, AnnotationEdit } from '../../api';
import { AnnotationStore, draft, type Entry } from './store';

function entry(uuid: string, progression: number | null, note = ''): Entry {
  return {
    annotation: { ...draft(1, 'highlight', 'yellow', note, 'citação', progression), uuid },
    marks: [],
    ranges: [],
    pending: false,
  };
}

function backend(fail: Set<string>) {
  const written: string[] = [];
  return {
    written,
    edit: async (uuid: string, edit: AnnotationEdit): Promise<Annotation> => {
      if (fail.has('edit')) throw new Error('recusado');
      written.push(`edit ${uuid} ${JSON.stringify(edit)}`);
      return { ...entry(uuid, 0.5).annotation, ...edit, updatedAt: 99 };
    },
    remove: async (uuid: string): Promise<Annotation> => {
      if (fail.has('remove')) throw new Error('recusado');
      written.push(`remove ${uuid}`);
      return { ...entry(uuid, 0.5).annotation, deletedAt: 1 };
    },
    restore: async (uuid: string): Promise<Annotation> => {
      written.push(`restore ${uuid}`);
      return entry(uuid, 0.5).annotation;
    },
  };
}

describe('the annotations shown', () => {
  it('are in reading order, orphans without a place last', () => {
    const store = new AnnotationStore(backend(new Set()), () => {});
    store.load([entry('b', 0.7), entry('c', null), entry('a', 0.1)]);
    expect(store.list().map((e) => e.annotation.uuid)).toEqual(['a', 'b', 'c']);
  });

  it('show a new one at once and swap in the written one, or take it back with the reason', async () => {
    const errors: string[] = [];
    const store = new AnnotationStore(backend(new Set()), (message) => errors.push(message));
    const shown = entry('provisional', 0.3);
    let release: (value: Entry) => void = () => {};
    const created = store.create(shown, () => new Promise((resolve) => (release = resolve)));
    expect(store.list()[0].pending).toBe(true);
    release(entry('written', 0.3));
    expect((await created)?.annotation.uuid).toBe('written');
    expect(store.list().map((e) => [e.annotation.uuid, e.pending])).toEqual([['written', false]]);

    const refused = await store.create(entry('other', 0.4), () => Promise.reject(new Error('sem biblioteca')));
    expect(refused).toBeNull();
    expect(store.list()).toHaveLength(1);
    expect(errors[0]).toContain('sem biblioteca');
  });

  it('put an edit or a removal back when the library refuses it', async () => {
    const errors: string[] = [];
    const store = new AnnotationStore(backend(new Set(['edit', 'remove'])), (message) => errors.push(message));
    store.load([entry('a', 0.1)]);
    await store.edit('a', { color: 'blue' });
    expect(store.get('a')?.annotation.color).toBe('yellow');
    await store.remove('a');
    expect(store.get('a')).toBeDefined();
    expect(errors).toHaveLength(2);
  });

  it('write edits, removals and restorations through the library', async () => {
    const library = backend(new Set());
    const store = new AnnotationStore(library, () => {});
    store.load([entry('a', 0.1)]);
    await store.edit('a', { note: '# nota' });
    expect(store.get('a')?.annotation.note).toBe('# nota');
    const removed = await store.remove('a');
    expect(store.list()).toHaveLength(0);
    await store.restore(removed as Entry);
    expect(store.list()).toHaveLength(1);
    expect(library.written).toEqual(['edit a {"note":"# nota"}', 'remove a', 'restore a']);
  });
});

describe('a mark on a scan', () => {
  const unread = (uuid: string): Entry => {
    const shown = entry(uuid, 0.2);
    return { ...shown, annotation: { ...shown.annotation, quote: null, quoteSource: 'none' } };
  };
  const read = (uuid: string): Entry['annotation'] => ({ ...unread(uuid).annotation, quote: 'As armas e os barões', quoteSource: 'ocr', color: 'stale' });

  it('learns the quote the OCR read after it was made, and only the quote', () => {
    const store = new AnnotationStore(backend(new Set()), () => {});
    store.load([unread('scan')]);
    store.learned(read('scan'));
    const [shown] = store.list();
    expect(shown.annotation.quote).toBe('As armas e os barões');
    expect(shown.annotation.quoteSource).toBe('ocr');
    expect(shown.annotation.color).toBe('yellow');
    // A quote, once there, stays.
    store.learned({ ...read('scan'), quote: 'outra' });
    expect(store.list()[0].annotation.quote).toBe('As armas e os barões');
  });

  it('keeps what was read before its writing came back', async () => {
    const store = new AnnotationStore(backend(new Set()), () => {});
    let release: (value: Entry) => void = () => {};
    const created = store.create(entry('provisional', 0.2), () => new Promise((resolve) => (release = resolve)));
    store.learned(read('scan'));
    release(unread('scan'));
    expect((await created)?.annotation.uuid).toBe('scan');
    expect(store.list()[0].annotation.quote).toBe('As armas e os barões');
  });
});
