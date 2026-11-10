/** The small part of foliate-js the adapter uses. It is plain JavaScript, with no types. */

declare module '*/foliate-js/vendor/zip.js' {
  export function configure(options: { useWebWorkers: boolean }): void;
  export class BlobReader {
    constructor(blob: Blob);
  }
  export class TextWriter {
    private readonly writesText: true;
  }
  export class BlobWriter {
    private readonly writesBlob: true;
    constructor(type?: string);
  }
  export interface ZipEntry {
    filename: string;
    uncompressedSize: number;
    getData(writer: TextWriter): Promise<string>;
    getData(writer: BlobWriter): Promise<Blob>;
  }
  export class ZipReader {
    constructor(reader: BlobReader);
    getEntries(): Promise<ZipEntry[]>;
  }
}

declare module '*/foliate-js/epub.js' {
  export interface EpubSection {
    id: string;
    size: number;
    linear?: string;
    cfi: string;
    load(): Promise<string>;
    unload(): void;
    resolveHref(href: string): string;
  }
  export interface EpubLoader {
    loadText(name: string): Promise<string | null>;
    loadBlob(name: string, type?: string): Promise<Blob | null> | null;
    getSize(name: string): number;
  }
  export class EPUB {
    constructor(loader: EpubLoader);
    init(): Promise<EPUB>;
    sections: EpubSection[];
    resolveHref(href: string): { index: number; anchor: (doc: Document) => Element | number | null } | null;
    resolveCFI(cfi: string): { index: number; anchor: (doc: Document) => Range | null };
  }
}

declare module '*/foliate-js/epubcfi.js' {
  export function joinIndir(...parts: string[]): string;
  export function fromRange(range: Range, filter?: unknown): string;
  export function parse(cfi: string): unknown[] & { parent?: unknown[] };
  export function toRange(doc: Document, parts: unknown[], filter?: unknown): Range;
}

declare module '*/foliate-js/paginator.js' {
  import type { EPUB } from '*/foliate-js/epub.js';

  export const sectionFrame: { sandbox: string };

  export class Paginator extends HTMLElement {
    open(book: EPUB): void;
    goTo(target: {
      index: number;
      anchor?: number | ((doc: Document) => Range | Element | number);
    }): Promise<void>;
    next(): Promise<void>;
    prev(): Promise<void>;
    firstSection(): Promise<void>;
    lastSection(): Promise<void>;
    setStyles(styles: string): void;
    render(): void;
    destroy(): void;
    getContents(): { index: number; doc: Document }[];
    readonly pages: number;
    readonly page: number;
    readonly size: number;
    readonly viewSize: number;
    readonly start: number;
    readonly atStart: boolean;
    readonly atEnd: boolean;
  }
}
