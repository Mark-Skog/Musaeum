/**
 * Annotations in and out of the library as W3C Web Annotations (JSON-LD), and study reports, for
 * the book open in the reader, the works chosen in the library, or the whole library. The reader's
 * panel, the details beside the books and the library's menu all run them through here, and say
 * what they did in the same words.
 */

import { api, type ExchangeScope, type ExchangeTarget, type JsonLdExported, type JsonLdImported, type ReportScope, type ReportWritten } from './api';

export type ExchangeAction = 'export-jsonld' | 'import-jsonld' | 'report-md' | 'report-pdf';

/** What the actions are about. */
export type ExchangeWhere =
  | { kind: 'document'; documentId: number }
  | { kind: 'works'; works: number[] }
  | { kind: 'library' };

export const EXCHANGE_ACTIONS: { id: ExchangeAction; label: string; busy: string }[] = [
  { id: 'export-jsonld', label: 'Exportar JSON-LD…', busy: 'Gravando as anotações em JSON-LD…' },
  { id: 'import-jsonld', label: 'Importar JSON-LD…', busy: 'Lendo e conferindo o JSON-LD…' },
  { id: 'report-md', label: 'Relatório em Markdown…', busy: 'Escrevendo o relatório…' },
  { id: 'report-pdf', label: 'Relatório em PDF…', busy: 'Compondo o relatório e conferindo o PDF…' },
];

/** What an action did: its words for the person, and what the host answered; `null` when cancelled. */
export interface ExchangeDone {
  text: string;
  result: JsonLdExported | JsonLdImported | ReportWritten;
}

const plural = (count: number, one: string, many: string): string => `${count} ${count === 1 ? one : many}`;

export function exportedText(report: JsonLdExported): string {
  const books = report.documents > 1 ? ` de ${plural(report.documents, 'livro', 'livros')}` : '';
  return `JSON-LD com ${plural(report.annotations, 'anotação', 'anotações')}${books} salvo em ${report.path}.`;
}

export function importedText(report: JsonLdImported): string {
  const parts: string[] = [`${plural(report.added, 'anotação trazida', 'anotações trazidas')}`];
  const found = { exact: 0, approximate: 0, orphan: 0, pending: 0 };
  for (const book of report.books) {
    found.exact += book.exact;
    found.approximate += book.approximate;
    found.orphan += book.orphan;
    found.pending += book.pending;
  }
  const how = [
    found.exact > 0 ? plural(found.exact, 'exata', 'exatas') : null,
    found.approximate > 0 ? plural(found.approximate, 'aproximada', 'aproximadas') : null,
    found.orphan > 0 ? plural(found.orphan, 'órfã', 'órfãs') : null,
    found.pending > 0 ? `${found.pending} a ancorar quando o livro abrir` : null,
  ].filter((part) => part !== null);
  if (report.added > 0 && how.length > 0) {
    parts[0] += ` (${how.join(', ')})`;
  }
  if (report.already > 0) {
    parts.push(`${report.already} já ${report.already === 1 ? 'estava' : 'estavam'} na biblioteca`);
  }
  if (report.elsewhere > 0) {
    parts.push(`${plural(report.elsewhere, 'é de outro livro', 'são de outros livros')} e ${report.elsewhere === 1 ? 'ficou' : 'ficaram'} de fora`);
  }
  const missing = report.unmatched.reduce((sum, work) => sum + work.annotations, 0);
  if (missing > 0) {
    const titles = report.unmatched.slice(0, 3).map((work) => `“${work.title}”`).join(', ');
    const more = report.unmatched.length > 3 ? ` e mais ${report.unmatched.length - 3}` : '';
    parts.push(`${plural(missing, 'é', 'são')} de obras que a biblioteca não tem (${titles}${more})`);
  }
  let text = parts.join('; ') + '.';
  if (report.otherWork > 0) {
    text += ` ${plural(report.otherWork, 'veio', 'vieram')} de outra obra, e ${report.otherWork === 1 ? 'foi trazida' : 'foram trazidas'} porque este livro foi escolhido: confira.`;
  }
  return text;
}

export function reportText(report: ReportWritten, format: 'markdown' | 'pdf'): string {
  const kind = format === 'pdf' ? `PDF, ${plural(report.pages ?? 0, 'página', 'páginas')}` : 'Markdown';
  const works = report.works > 1 ? ` de ${plural(report.works, 'livro', 'livros')}` : '';
  return `Relatório (${kind}) com ${plural(report.annotations, 'anotação', 'anotações')}${works} salvo em ${report.path}.`;
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
        throw new Error('importe as anotações de uma obra por vez, ou na biblioteca toda');
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
      throw new Error('um relatório é de livros escolhidos');
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
