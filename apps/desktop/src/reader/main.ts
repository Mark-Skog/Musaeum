/**
 * The reader, running in its own iframe at its own origin. It has the book's bytes and the
 * means to lay them out, and it has no way to reach the app's IPC: the only thing it can do is
 * send the main page the few messages in `messages.ts`.
 */

import { type FluidEngine, openEpub } from './foliate-adapter';
import { pickSections, runLayoutBench } from './layout-bench';
import type { FromReader, ReaderScenario, ToReader } from './messages';
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

async function main(): Promise<void> {
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

function fail(error: unknown): void {
  send({
    type: 'failed',
    message: error instanceof Error ? (error.stack ?? error.message) : String(error),
  });
}

main().catch(fail);
