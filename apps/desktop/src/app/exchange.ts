/**
 * Annotations in and out of the library as W3C Web Annotations (JSON-LD), and study reports, for
 * the book open in the reader, the works chosen in the library, or the whole library. The reader's
 * panel, the details beside the books and the library's menu all run them through here, and say
 * what they did in the same words.
 */

import { api, type ExchangeScope, type ExchangeTarget, type JsonLdExported, type JsonLdImported, type ReportScope, type ReportWritten } from './api';
import { type MessageKey, t, tn } from './i18n';

export type ExchangeAction = 'export-jsonld' | 'import-jsonld' | 'report-md' | 'report-pdf';

/** What the actions are about. */
export type ExchangeWhere =
  | { kind: 'document'; documentId: number }
  | { kind: 'works'; works: number[] }
  | { kind: 'library' };

/** Each action, with the words of its button and of its status while it runs. */
export const EXCHANGE_ACTIONS: { id: ExchangeAction; label: MessageKey; busy: MessageKey }[] = [
  { id: 'export-jsonld', label: 'exchange.export', busy: 'exchange.export.busy' },
  { id: 'import-jsonld', label: 'exchange.import', busy: 'exchange.import.busy' },
  { id: 'report-md', label: 'exchange.reportMd', busy: 'exchange.reportMd.busy' },
  { id: 'report-pdf', label: 'exchange.reportPdf', busy: 'exchange.reportPdf.busy' },
];

/** What an action did: its words for the person, and what the host answered; `null` when cancelled. */
export interface ExchangeDone {
  text: string;
  result: JsonLdExported | JsonLdImported | ReportWritten;
}

export function exportedText(report: JsonLdExported): string {
  const annotations = tn('exchange.annotations', report.annotations);
  return report.documents > 1
    ? t('exchange.exported.books', { annotations, books: tn('exchange.books', report.documents), path: report.path })
    : t('exchange.exported', { annotations, path: report.path });
}

export function importedText(report: JsonLdImported): string {
  const parts: string[] = [tn('exchange.brought', report.added)];
  const found = { exact: 0, approximate: 0, orphan: 0, pending: 0 };
  for (const book of report.books) {
    found.exact += book.exact;
    found.approximate += book.approximate;
    found.orphan += book.orphan;
    found.pending += book.pending;
  }
  const how = [
    found.exact > 0 ? tn('exchange.exact', found.exact) : null,
    found.approximate > 0 ? tn('exchange.approximate', found.approximate) : null,
    found.orphan > 0 ? tn('exchange.orphan', found.orphan) : null,
    found.pending > 0 ? t('exchange.pending', { count: found.pending }) : null,
  ].filter((part) => part !== null);
  if (report.added > 0 && how.length > 0) {
    parts[0] += ` (${how.join(', ')})`;
  }
  if (report.already > 0) {
    parts.push(tn('exchange.already', report.already));
  }
  if (report.elsewhere > 0) {
    parts.push(tn('exchange.elsewhere', report.elsewhere));
  }
  const missing = report.unmatched.reduce((sum, work) => sum + work.annotations, 0);
  if (missing > 0) {
    const titles = report.unmatched.slice(0, 3).map((work) => `“${work.title}”`).join(', ');
    const more = report.unmatched.length > 3 ? ` ${t('exchange.more', { count: report.unmatched.length - 3 })}` : '';
    parts.push(tn('exchange.unmatched', missing, { titles: `${titles}${more}` }));
  }
  let text = parts.join('; ') + '.';
  if (report.otherWork > 0) {
    text += ` ${tn('exchange.otherWork', report.otherWork)}`;
  }
  return text;
}

export function reportText(report: ReportWritten, format: 'markdown' | 'pdf'): string {
  const kind = format === 'pdf' ? t('exchange.kind.pdf', { pages: tn('exchange.pages', report.pages ?? 0) }) : 'Markdown';
  const annotations = tn('exchange.annotations', report.annotations);
  return report.works > 1
    ? t('exchange.report.books', { kind, annotations, books: tn('exchange.books', report.works), path: report.path })
    : t('exchange.report', { kind, annotations, path: report.path });
}

function scopeOf(where: ExchangeWhere): ExchangeScope {
  switch (where.kind) {
    case 'document':
      return { kind: 'document', ids: where.documentId };
    case 'works':
      return { kind: 'works', ids: where.works };
    case 'library':
      return { kind: 'library' };
  }
}

function targetOf(where: ExchangeWhere): ExchangeTarget {
  switch (where.kind) {
    case 'document':
      return { kind: 'document', id: where.documentId };
    case 'works':
      if (where.works.length !== 1) {
        throw new Error(t('exchange.importOneWork'));
      }
      return { kind: 'work', id: where.works[0] };
    case 'library':
      return { kind: 'library' };
  }
}

function reportScopeOf(where: ExchangeWhere): ReportScope {
  switch (where.kind) {
    case 'document':
      return { kind: 'document', ids: where.documentId };
    case 'works':
      return { kind: 'works', ids: where.works };
    case 'library':
      throw new Error(t('exchange.reportChosen'));
  }
}

/** Runs an action: the system's dialog asks where, unless a test gives the path. */
export async function runExchange(action: ExchangeAction, where: ExchangeWhere, path?: string, generatedAt?: number): Promise<ExchangeDone | null> {
  switch (action) {
    case 'export-jsonld': {
      const report = await api.exportJsonLd(scopeOf(where), path);
      return report === null ? null : { text: exportedText(report), result: report };
    }
    case 'import-jsonld': {
      const report = await api.importJsonLd(targetOf(where), path);
      return report === null ? null : { text: importedText(report), result: report };
    }
    case 'report-md':
    case 'report-pdf': {
      const format = action === 'report-md' ? 'markdown' : 'pdf';
      const report = await api.studyReport(format, reportScopeOf(where), path, generatedAt);
      return report === null ? null : { text: reportText(report, format), result: report };
    }
  }
}

/** What an action did, or why it could not, for a test. */
export type ExchangeOutcome = { ok: true; done: ExchangeDone | null } | { ok: false; error: string };

export function reasonText(reason: unknown): string {
  return String(reason instanceof Error ? reason.message : reason);
}
