import { describe, expect, it } from 'vitest';

import { EN_AREAS } from './en';
import { CATALOGS, fill, formatBytes, formatNumber, isLocale, setLocale, t, tIn, tn } from './index';
import { PT_AREAS } from './pt-BR';

const names = (template: string): string[] => [...new Set([...template.matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g)].map((match) => match[1]))].sort();

describe('the catalogs', () => {
  it('have the same areas, and in each the same keys', () => {
    expect(Object.keys(EN_AREAS).sort()).toEqual(Object.keys(PT_AREAS).sort());
    for (const [area, keys] of Object.entries(PT_AREAS)) {
      expect(Object.keys(EN_AREAS[area as keyof typeof EN_AREAS]).sort(), area).toEqual(Object.keys(keys).sort());
    }
  });

  it('never have one key in two areas', () => {
    const seen = new Map<string, string>();
    for (const [area, keys] of Object.entries(PT_AREAS)) {
      for (const key of Object.keys(keys)) {
        expect(seen.get(key), `${key} is in ${area} and ${seen.get(key)}`).toBeUndefined();
        seen.set(key, area);
        // A key starts with its area's prefix, so that two areas cannot reach for the same one.
        expect(key.includes('.'), key).toBe(true);
      }
    }
  });

  it('take the same values in both languages, and nothing is empty', () => {
    for (const [key, portuguese] of Object.entries(CATALOGS['pt-BR'])) {
      const english = CATALOGS.en[key as keyof typeof CATALOGS.en];
      expect(names(english), key).toEqual(names(portuguese));
      for (const text of [portuguese, english]) {
        expect(text.trim().length, key).toBeGreaterThan(0);
        expect(text, key).toBe(text.trim());
      }
    }
  });

  it('give every count both its forms', () => {
    const keys = Object.keys(CATALOGS['pt-BR']);
    for (const key of keys) {
      if (key.endsWith('.one')) {
        expect(keys, key).toContain(`${key.slice(0, -4)}.other`);
      }
      if (key.endsWith('.other')) {
        expect(keys, key).toContain(`${key.slice(0, -6)}.one`);
      }
    }
  });

  it('leave no Portuguese untranslated in English', () => {
    for (const [key, english] of Object.entries(CATALOGS.en)) {
      expect(/[ãõçáéíóúâêôà]/i.test(english), `${key}: ${english}`).toBe(false);
    }
  });
});

describe('a message', () => {
  it('is written in the language shown, with its values put in', () => {
    setLocale('en');
    expect(t('prefs.cache.total', { folder: 'C:\\x' })).toBe('Everything, in C:\\x');
    expect(tn('prefs.cache.pages', 1)).toBe('1 page');
    expect(tn('prefs.cache.pages', 12345)).toBe('12,345 pages');
    setLocale('pt-BR');
    expect(t('prefs.cache.total', { folder: 'C:\\x' })).toBe('Tudo, em C:\\x');
    expect(tn('prefs.cache.pages', 0)).toBe('0 páginas');
    expect(tn('prefs.cache.pages', 12345)).toBe('12.345 páginas');
    expect(tIn('en', 'common.close')).toBe('Close');
  });

  it('keeps braces that name nothing, and a name with no value as it was', () => {
    expect(fill('{ a } {1} {b}', { a: 'x' })).toBe('{ a } {1} {b}');
    expect(fill('{a}{a}', { a: 'y' })).toBe('yy');
  });

  it('writes numbers and sizes as the language does, years without a separator', () => {
    expect(formatNumber(2026, 'pt-BR')).toBe('2026');
    expect(formatNumber(12345.5, 'pt-BR')).toBe('12.345,5');
    expect(formatNumber(12345.5, 'en')).toBe('12,345.5');
    expect(formatBytes(0, 'pt-BR')).toBe('0 B');
    expect(formatBytes(1536, 'pt-BR')).toBe('1,5 KB');
    expect(formatBytes(1536, 'en')).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 * 1024 * 1024, 'en')).toBe('5.0 GB');
    expect(isLocale('en') && isLocale('pt-BR') && !isLocale('es')).toBe(true);
  });
});
