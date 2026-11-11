/**
 * The reader, running in its own iframe at its own origin. It has the book's files and the means
 * to lay them out, and it has no way to reach the app's IPC: the only thing it can do is send the
 * main page the few messages in `messages.ts`.
 *
 * It opens a book two ways. A book of the library comes normalized, file by file, from `pkg/`
 * (`?package=1`): that is the app's reader. An EPUB named on the command line comes as one file,
 * `book.epub`, for the benchmarks of Phases 0 and 3, which measure the engine on the raw book.
 */

import { EPUB } from '../../../../vendor/foliate-js/epub.js';
import { sectionFrame } from '../../../../vendor/foliate-js/paginator.js';
import { type FluidEngine, openEpub } from './foliate-adapter';
import { FlowEngine, type FlowBook } from './flow-engine';
import { pickSections, runLayoutBench } from './layout-bench';
import { type FromReader, isToReader, type ReaderScenario, type ToReader } from './messages';
import { packageLoader } from './package-loader';
import { scanSections } from './security-scan';
import type { Typography } from './typography';

const BASE_TYPOGRAPHY: Typography = { fontSizePx: 18, lineHeight: 1.5 };
/** How long the memory scenario holds each state before sampling it. */
const MEMORY_SETTLE_MS = 15_000;
const MEMORY_READING_MS = 30_000;
const PAGE_TURN_MS = 400;

const parameters = new URLSearchParams(location.search);
const hostOrigin = parameters.get('host') ?? '';
const sectionSandbox = parameters.get('sections') ?? 'allow-same-origin';
const isPackage = parameters.get('package') === '1';
const e2e = parameters.get('e2e') === '1';

function send(message: FromReader): void {
  parent.postMessage(message, hostOrigin);
}

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

let nextResizeId = 1;
const pendingResizes = new Map<number, () => void>();

function resizeWindow(width: number, height: number): Promise<void> {
  const id = nextResizeId++;
  return new Promise((resolve) => {
    pendingResizes.set(id, resolve);
    send({ type: 'resize-window', id, width, height });
  });
}

async function runMemoryScenario(engine: FluidEngine): Promise<void> {
  send({ type: 'phase', name: 'opened' });
  await pause(MEMORY_SETTLE_MS);

  send({ type: 'phase', name: 'reading' });
  const ends = performance.now() + MEMORY_READING_MS;
  while (performance.now() < ends) {
    await engine.nextPage();
    await pause(PAGE_TURN_MS);
  }

  send({ type: 'phase', name: 'idle-after-reading' });
  await pause(MEMORY_SETTLE_MS);
  send({ type: 'phase', name: 'end' });
}

async function runScenario(engine: FluidEngine, scenario: ReaderScenario): Promise<void> {
  switch (scenario) {
    case 'interactive':
      window.addEventListener('keydown', (event) => {
        if (event.key === 'ArrowRight') {
          void engine.nextPage();
        } else if (event.key === 'ArrowLeft') {
          void engine.previousPage();
        }
      });
      return;
    case 'smoke': {
      await engine.goToSection(0);
      const doc = engine.currentDocument();
      send({
        type: 'report',
        report: { hasText: (doc?.body?.textContent?.length ?? 0) > 100, sections: engine.sectionCount },
      });
      return;
    }
    case 'layout': {
      const measurements = await runLayoutBench(
        { engine, typography: BASE_TYPOGRAPHY, resizeWindow },
        pickSections(engine.sectionSizes),
      );
      send({ type: 'report', report: { measurements } });
      return;
    }
    case 'security': {
      const sections = await scanSections(engine);
      send({ type: 'report', report: { sections, sectionSandbox } });
      return;
    }
    case 'memory':
      await runMemoryScenario(engine);
      send({ type: 'report', report: {} });
      return;
  }
}

/** The benchmarks' reader: one EPUB file, laid out by foliate-js as it is. */
async function openRawBook(): Promise<void> {
  const started = performance.now();
  const response = await fetch('book.epub', { cache: 'no-store', credentials: 'omit' });
  if (!response.ok) {
    throw new Error(`the host answered ${response.status} for the book`);
  }
  const engine = await openEpub(await response.blob(), document.body, sectionSandbox);
  await engine.goToSection(0);
  const openMs = performance.now() - started;

  window.addEventListener('message', (event: MessageEvent<ToReader>) => {
    // A section shares this origin, so only the page above is believed.
    if (event.source !== parent) {
      return;
    }
    const message = event.data;
    if (message.type === 'run') {
      runScenario(engine, message.scenario).catch(fail);
    } else if (message.type === 'window-resized') {
      pendingResizes.get(message.id)?.();
      pendingResizes.delete(message.id);
    }
  });
  send({ type: 'ready', sections: engine.sectionCount, openMs });
}

/** The app's reader: a normalized book of the library. */
async function openPackage(): Promise<void> {
  const started = performance.now();
  sectionFrame.sandbox = sectionSandbox;
  const book = (await new EPUB(await packageLoader()).init()) as unknown as FlowBook;
  const container = document.createElement('div');
  container.id = 'book';
  document.body.append(container);
  let engine: FlowEngine | null = null;

  window.addEventListener('message', (event: MessageEvent<unknown>) => {
    // A section shares this origin, so only the page above is believed.
    if (event.source !== parent || !isToReader(event.data)) {
      return;
    }
    const message = event.data;
    switch (message.type) {
      case 'open': {
        if (engine !== null) {
          return;
        }
        engine = new FlowEngine(
          book,
          container,
          message.sections,
          message.totalText,
          {
            mode: message.mode,
            animated: message.animated,
            style: message.style,
            sandbox: sectionSandbox,
            reducedMotion: message.reducedMotion,
          },
          send,
        );
        engine
          .open(message.position)
          .then(() => send({ type: 'opened', openMs: performance.now() - started }))
          .catch(fail);
        return;
      }
      case 'set-mode':
        void engine?.setMode(message.mode).catch(fail);
        return;
      case 'set-animation':
        engine?.setAnimated(message.animated);
        return;
      case 'set-style':
        void engine?.setStyle(message.style).catch(fail);
        return;
      case 'go-to':
        void engine?.goTo(message.target).catch(fail);
        return;
      case 'turn':
        void engine?.turn(message.step).catch(fail);
        return;
      case 'key':
        engine?.key(message.key, message.shift);
        return;
      case 'marks':
        engine?.setMarks(message.marks);
        return;
      case 'clear-selection':
        engine?.clearSelection();
        return;
      default:
        return;
    }
  });
  window.addEventListener('keydown', (event) => engine?.onKey(event));

  if (e2e) {
    // For tests that drive the real window from outside: what the reader holds, exactly.
    (window as unknown as { musaeumReaderE2E: unknown }).musaeumReaderE2E = {
      inspect: () => engine?.inspect() ?? null,
      anchor: () => engine?.currentAnchor() ?? null,
      idle: () => engine?.idle() ?? Promise.resolve(),
      goToFraction: (fraction: number) => engine?.goTo({ fraction }) ?? Promise.resolve(),
      goToSection: (index: number) => engine?.goToSection(index) ?? Promise.resolve(),
      cuts: () => engine?.cuts() ?? [],
      seams: () => engine?.seams() ?? [],
      plan: (index: number) => engine?.plan(index) ?? null,
      clickLink: (href: string) => engine?.clickLink(href) ?? false,
      select: (start: string, end: string) => engine?.annotations.select(start, end) ?? false,
      marks: () => engine?.annotations.inspect() ?? null,
      probeSelection: (seam: boolean, needle: string | null = null) => engine?.annotations.probe(seam, needle) ?? null,
    };
  }
  send({ type: 'ready', sections: book.sections.length, openMs: performance.now() - started });
}

function fail(error: unknown): void {
  send({
    type: 'failed',
    message: error instanceof Error ? (error.stack ?? error.message) : String(error),
  });
}

(isPackage ? openPackage() : openRawBook()).catch(fail);
