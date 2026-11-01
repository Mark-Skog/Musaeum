/**
 * The fluid engine, behind an interface of our own. This is the only file that imports
 * foliate-js, which declares itself unstable, so replacing it touches nothing else.
 */

import { EPUB, type EpubLoader } from '../../../../vendor/foliate-js/epub.js';
import { type Paginator, sectionFrame } from '../../../../vendor/foliate-js/paginator.js';
import {
  BlobReader,
  BlobWriter,
  configure,
  TextWriter,
  ZipReader,
} from '../../../../vendor/foliate-js/vendor/zip.js';
import { typographyCss, type Typography } from './typography';

export type { Typography } from './typography';

export type FluidFlow = 'paginated' | 'scrolled';

export interface FluidEngine {
  readonly sectionCount: number;
  /** The uncompressed size of each section, in spine order. */
  readonly sectionSizes: readonly number[];
  /** Opens a section at `fraction` of its length, from 0 to 1. */
  goToSection(index: number, fraction?: number): Promise<void>;
  nextPage(): Promise<void>;
  previousPage(): Promise<void>;
  setTypography(typography: Typography): void;
  setFlow(flow: FluidFlow): void;
  /** Resizes the engine inside its page, or returns it to filling the page with `null`. */
  setBoxWidth(widthPx: number | null): void;
  /** The current section's document, which lives in its own iframe. */
  currentDocument(): Document | null;
  /** The text on screen, as of the last time the engine settled on a position. */
  readingPosition(): Range | null;
  /** Numbers that change when the book's layout does. */
  layoutFingerprint(): string;
}

async function loaderFromFile(file: Blob): Promise<EpubLoader> {
  // Workers would need a looser Content Security Policy, and are not needed: the browser
  // decompresses entries off the main thread on its own.
  configure({ useWebWorkers: false });
  const reader = new ZipReader(new BlobReader(file));
  const entries = new Map((await reader.getEntries()).map((entry) => [entry.filename, entry]));
  return {
    loadText: async (name) => (await entries.get(name)?.getData(new TextWriter())) ?? null,
    loadBlob: (name, type) => entries.get(name)?.getData(new BlobWriter(type)) ?? null,
    getSize: (name) => entries.get(name)?.uncompressedSize ?? 0,
  };
}

/**
 * Opens an EPUB in `container`.
 *
 * `sectionSandbox` is the `sandbox` attribute of the iframe of each section. Scripts must stay
 * out of it wherever the browser allows that, which Chromium does.
 */
export async function openEpub(
  file: Blob,
  container: HTMLElement,
  sectionSandbox: string,
): Promise<FluidEngine> {
  sectionFrame.sandbox = sectionSandbox;
  const book = await new EPUB(await loaderFromFile(file)).init();

  const paginator = document.createElement('foliate-paginator') as Paginator;
  paginator.open(book);
  container.append(paginator);

  let typography = typographyCss({ fontSizePx: 18, lineHeight: 1.5 });
  paginator.setStyles(typography);

  let position: Range | null = null;
  paginator.addEventListener('relocate', (event) => {
    position = (event as CustomEvent<{ range: Range | null }>).detail.range;
  });

  return {
    sectionCount: book.sections.length,
    sectionSizes: book.sections.map((section) => section.size),
    async goToSection(index, fraction = 0) {
      await paginator.goTo({ index, anchor: fraction });
    },
    nextPage: () => paginator.next(),
    previousPage: () => paginator.prev(),
    setTypography(next) {
      typography = typographyCss(next);
      paginator.setStyles(typography);
    },
    setFlow(flow) {
      paginator.setAttribute('flow', flow);
    },
    setBoxWidth(widthPx) {
      paginator.style.width = widthPx === null ? '' : `${widthPx}px`;
    },
    currentDocument: () => paginator.getContents()[0]?.doc ?? null,
    readingPosition: () => position,
    layoutFingerprint: () => `${paginator.size}/${paginator.viewSize}/${paginator.start}`,
  };
}
