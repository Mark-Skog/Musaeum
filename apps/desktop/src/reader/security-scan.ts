/**
 * Walks a hostile book and records what its content managed to do.
 *
 * The verdict does not rely on the content telling us: every section is checked from outside,
 * by the trusted reader. A script that ran anywhere in a section leaves its mark on the
 * document, which is what `data-pwned` looks for. The canary server and the app's own message
 * listener are watched separately, by the test harness and the main page.
 */

import type { FluidEngine } from './foliate-adapter';

export interface SectionScan {
  index: number;
  name: string;
  /** The section's own text arrived, so it was really loaded. */
  loaded: boolean;
  /** Which scripts marked the document, if any. */
  scriptMark: string | null;
}

/** Long enough for an event handler, an animation or a nested frame to have run. */
const SECTION_DWELL_MS = 450;

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Does what a person reading the book would do with the parts that need a person. */
function poke(doc: Document): void {
  for (const element of doc.querySelectorAll<HTMLElement>('[data-poke]')) {
    try {
      if (element.dataset.poke === 'click') {
        element.click();
      } else if (element instanceof doc.defaultView!.HTMLFormElement) {
        element.requestSubmit();
      }
    } catch (error) {
      console.info('the sandbox refused a poke', error);
    }
  }
}

export async function scanSections(engine: FluidEngine): Promise<SectionScan[]> {
  const scans: SectionScan[] = [];
  for (let index = 0; index < engine.sectionCount; index++) {
    await engine.goToSection(index);
    const doc = engine.currentDocument();
    if (doc === null) {
      scans.push({ index, name: `section ${index}`, loaded: false, scriptMark: null });
      continue;
    }
    poke(doc);
    await pause(SECTION_DWELL_MS);

    // The document may have been navigated away by now, which is one of the attacks.
    const current = engine.currentDocument() ?? doc;
    const loaded = current.getElementById('chapter-loaded') !== null;
    scans.push({
      index,
      name: current.title || `section ${index}`,
      loaded,
      scriptMark: current.documentElement?.getAttribute('data-pwned') ?? null,
    });
  }
  return scans;
}
