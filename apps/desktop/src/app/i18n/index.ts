/**
 * The languages of the interface: every word it shows comes from a catalog, Brazilian Portuguese
 * (`pt-BR.ts`, the first) and English (`en.ts`), with the same keys and the same values named in
 * braces (`{count}`), which the compiler and `i18n.test.ts` hold.
 *
 * The language is the person's, from the preferences, and changes at once: components that show
 * words read them through `useT()`, which redraws them when it changes; code outside components
 * calls `t()` when it runs. A count picks one of two messages, `key.one` for exactly one and
 * `key.other` for any other number, as the Rust side counts too.
 */

import { useSyncExternalStore } from 'react';

import { en } from './en';
import { type MessageKey, type Messages, ptBR } from './pt-BR';

export type { MessageKey } from './pt-BR';

export type Locale = 'pt-BR' | 'en';

export const LOCALES: readonly Locale[] = ['pt-BR', 'en'];

const catalogs: Record<Locale, Messages> = { 'pt-BR': ptBR, en };

/** A key with a `.one` and an `.other`. */
export type CountKey = MessageKey extends infer K ? (K extends `${infer Base}.one` ? Base : never) : never;

export type Values = Record<string, string | number>;

let current: Locale = 'pt-BR';
const listeners = new Set<() => void>();

/** Whether `value` names a language of the interface. */
export function isLocale(value: unknown): value is Locale {
  return value === 'pt-BR' || value === 'en';
}

/** Makes `locale` the language of everything shown from now on, redrawing what is shown. */
export function setLocale(locale: Locale): void {
  if (locale === current) {
    return;
  }
  current = locale;
  if (typeof document !== 'undefined') {
    document.documentElement.lang = locale;
  }
  for (const listener of listeners) {
    listener();
  }
}

export function getLocale(): Locale {
  return current;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The language shown now, redrawing the component when it changes. */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribe, getLocale, getLocale);
}

/** `t`, for a component: it is drawn again when the language changes. */
export function useT(): typeof t {
  useLocale();
  return t;
}

const numberFormats = new Map<Locale, Intl.NumberFormat>();

/** A number as the language writes it: 1.234,5 or 1,234.5, grouped only from five digits on. */
export function formatNumber(value: number, locale: Locale = current): string {
  let format = numberFormats.get(locale);
  if (format === undefined) {
    format = new Intl.NumberFormat(locale, { maximumFractionDigits: 2, useGrouping: 'min2' });
    numberFormats.set(locale, format);
  }
  return format.format(value);
}

/** Puts each `{name}` of `template` in from `values`; numbers as the language writes them. */
export function fill(template: string, values: Values | undefined, locale: Locale = current): string {
  if (values === undefined) {
    return template;
  }
  return template.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (whole, name: string) => {
    const value = values[name];
    if (value === undefined) {
      return whole;
    }
    return typeof value === 'number' ? formatNumber(value, locale) : value;
  });
}

/** The message `key` in the language shown now, its values put in. */
export function t(key: MessageKey, values?: Values): string {
  return fill(catalogs[current][key], values);
}

/** The message `key` in `locale`. */
export function tIn(locale: Locale, key: MessageKey, values?: Values): string {
  return fill(catalogs[locale][key], values, locale);
}

/** One of `key.one` and `key.other` by `count`, which is also put in as `{count}`. */
export function tn(key: CountKey, count: number, values?: Values): string {
  return t(`${key}.${count === 1 ? 'one' : 'other'}` as MessageKey, { count, ...values });
}

/** A byte count as a person reads it: 0 B, 12 KB, 3,4 MB, 1,2 GB. */
export function formatBytes(bytes: number, locale: Locale = current): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: digits, minimumFractionDigits: digits, useGrouping: 'min2' }).format(value)} ${units[unit]}`;
}

/** A date as the language writes it, from milliseconds since 1970. */
export function formatDate(ms: number, style: 'date' | 'date-time' = 'date', locale: Locale = current): string {
  return new Intl.DateTimeFormat(locale, style === 'date' ? { dateStyle: 'short' } : { dateStyle: 'short', timeStyle: 'short' }).format(ms);
}

/** Compares names as the language sorts them. */
export function compareNames(a: string, b: string, locale: Locale = current): number {
  return a.localeCompare(b, locale);
}

/** A language's own name in the language shown, from its BCP 47 tag (`pt`, `es`). */
export function languageName(bcp47: string, locale: Locale = current): string {
  try {
    const name = new Intl.DisplayNames([locale], { type: 'language' }).of(bcp47);
    return name === undefined ? bcp47 : name.charAt(0).toLocaleUpperCase(locale) + name.slice(1);
  } catch {
    return bcp47;
  }
}

/**
 * `key`'s message as written, its `{names}` still in it, for code that fills it where `t` cannot run:
 * the reader's frame, which has no catalog of its own.
 */
export function template(key: MessageKey, locale: Locale = current): string {
  return catalogs[locale][key];
}

/** The catalogs, for the tests that hold them together. */
export const CATALOGS = catalogs;
