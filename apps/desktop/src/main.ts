/**
 * The main page. With no document on the command line, it is the app: the library, and the
 * reader a book opens in. With one, it is the page of Phases 0 to 2, which opens that document
 * right away for the benchmarks and the tests that drive the window: those keep measuring exactly
 * what they measured before.
 */

import { invoke } from '@tauri-apps/api/core';

import { runBook } from './book-host';
import type { ReaderStatus, ToWorker } from './compositor/messages';
import { FixedReader } from './fixed/fixed-reader';
import type { BenchOptions, StartupOptions } from './protocol';

/** How often the main-thread stress test blocks the thread. */
const STRESS_PERIOD_MS = 200;
/** How long the memory scenario holds each resting state. */
const MEMORY_SETTLE_MS = 15_000;
const MEMORY_READING_MS = 30_000;
const MEMORY_SCROLL_EVERY_MS = 1000;

/**
 * The reading bar: the mode, the page turn, facing pages and the zoom. It only sends what was
 * clicked and shows what the compositor reports, so it can never disagree with the reader.
 */
function connectReadingBar(send: (message: ToWorker) => void): (status: ReaderStatus) => void {
  const bar = document.getElementById('reading-bar') as HTMLElement;
  const paginated = document.getElementById('mode-paginated') as HTMLButtonElement;
  const continuous = document.getElementById('mode-continuous') as HTMLButtonElement;
  const animation = document.getElementById('page-turn') as HTMLInputElement;
  const spreads = document.getElementById('spreads') as HTMLSelectElement;
  const zoom = document.getElementById('zoom-level') as HTMLOutputElement;
  const page = document.getElementById('page-number') as HTMLOutputElement;
  bar.hidden = false;

  paginated.addEventListener('click', () => send({ type: 'set-mode', mode: 'paginated' }));
  continuous.addEventListener('click', () => send({ type: 'set-mode', mode: 'continuous' }));
  animation.addEventListener('change', () =>
    send({ type: 'set-animation', on: animation.checked }),
  );
  spreads.addEventListener('change', () =>
    send({ type: 'set-spreads', spreads: spreads.value as 'auto' | 'single' | 'double' }),
  );
  const click = (id: string, message: ToWorker): void =>
    document.getElementById(id)?.addEventListener('click', () => send(message));
  click('zoom-out', { type: 'zoom', factor: 1 / 1.25 });
  click('zoom-in', { type: 'zoom', factor: 1.25 });
  click('page-back', { type: 'turn', step: -1 });
  click('page-forward', { type: 'turn', step: 1 });

  return (status) => {
    paginated.setAttribute('aria-pressed', String(status.mode === 'paginated'));
    continuous.setAttribute('aria-pressed', String(status.mode === 'continuous'));
    // Continuous reading has no page turn to animate, so the switch is off and out of reach there.
    animation.checked = status.animating;
    animation.disabled = status.mode === 'continuous';
    spreads.value = status.spreads;
    zoom.value = `${Math.round(status.zoom * 100)}%`;
    page.value = `${status.page + 1} / ${status.pageCount}`;
  };
}

/** Blocks the main thread periodically, the way a heavy UI update would. */
function startMainThreadStress(busyMs: number): void {
  if (busyMs > 0) {
    setInterval(() => {
      const end = performance.now() + busyMs;
      while (performance.now() < end) {
        // Deliberately spinning.
      }
    }, STRESS_PERIOD_MS);
  }
}

async function failBench(bench: BenchOptions | null, message: string): Promise<void> {
  console.error(message);
  const hud = document.getElementById('hud');
  if (hud !== null) {
    hud.textContent = message;
  }
  if (bench !== null) {
    await invoke('bench_fail', { message });
  }
}

function pause(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * The memory scenario: a person opening a document, reading for a while, and stepping away. The
 * compositor runs as it would for that person, going idle when nothing moves, which a scripted
 * scroll scenario would not let it do. Without `scroll`, there is nothing open to read.
 */
async function runMemoryScenario(
  scroll: ((deltaPx: number) => void) | null,
  extra: () => Record<string, unknown>,
): Promise<void> {
  const mark = (name: string): Promise<void> => invoke('bench_mark_phase', { name });
  await mark(scroll === null ? 'empty' : 'opened');
  await pause(MEMORY_SETTLE_MS);
  if (scroll !== null) {
    await mark('reading');
    const ends = performance.now() + MEMORY_READING_MS;
    while (performance.now() < ends) {
      scroll(window.innerHeight * devicePixelRatio * 0.75);
      await pause(MEMORY_SCROLL_EVERY_MS);
    }
    await mark('idle-after-reading');
    await pause(MEMORY_SETTLE_MS);
  }
  await mark('end');
  await invoke('bench_finish', { frontend: JSON.stringify(extra()) });
}

/** The page of Phases 0 to 2: one document, opened at start. */
async function runDocument(options: StartupOptions): Promise<void> {
  const hud = document.getElementById('hud') as HTMLPreElement;
  const canvas = document.getElementById('surface') as HTMLCanvasElement;
  const { bench } = options;
  const measuringMemory = bench?.scenario === 'memory';

  if (options.epub !== null) {
    await runBook(options, (message) => void failBench(bench, message));
    return;
  }
  if (options.document === null) {
    hud.textContent = 'Nada a medir.';
    if (measuringMemory) {
      await runMemoryScenario(null, () => ({}));
    }
    return;
  }

  const nowEpochMs = (): number => performance.timeOrigin + performance.now();
  const openRequestedAt = nowEpochMs();
  const uptimeWhenOpening = await invoke<number>('uptime_ms');
  let opening: Record<string, number> = {};
  let showStatus: ((status: ReaderStatus) => void) | null = null;
  let exposeTestHooks: (() => void) | null = null;
  let compositorReady = false;
  let reader: FixedReader | null = null;

  reader = await FixedReader.open({
    options,
    canvas,
    document: options.document,
    shortcuts: true,
    bench,
    onStatus: (status) => showStatus?.(status),
    onHud: (text) => {
      hud.textContent = text;
    },
    onFatal: (message) => void failBench(bench, message),
    onFirstSharp: (atEpochMs) => {
      opening = {
        // From asking for the document, which is what a reader who is already in the app waits.
        openToFirstSharpMs: atEpochMs - openRequestedAt,
        // From the program starting, which is what someone opening a file from outside waits.
        coldStartToFirstSharpMs: uptimeWhenOpening + (atEpochMs - openRequestedAt),
      };
    },
    onReady: () => {
      compositorReady = true;
      exposeTestHooks?.();
    },
    onBenchDone: (report) => {
      void invoke('bench_finish', {
        frontend: JSON.stringify({
          ...report,
          ...reader?.report,
          ...opening,
          userAgent: navigator.userAgent,
          devicePixelRatio,
          hardwareConcurrency: navigator.hardwareConcurrency,
          crossOriginIsolated,
          mainThreadCopyMs: reader?.copySamples.summary(),
          mainThreadFrameIntervalMs: reader?.mainFrameIntervals.summary(),
          badSlots: reader?.badSlots() ?? 0,
        }),
      });
    },
  });
  const opened = reader;
  if (bench === null) {
    showStatus = connectReadingBar((message) => opened.send(message));
  }

  if (options.e2e) {
    // For tests that drive this window from outside, through the WebView's debugging protocol:
    // what the reader holds, exactly, and nothing that would let them change it but the input a
    // person would give. They appear only once the compositor runs: a question asked before that
    // would be dropped, and the test would wait for its answer forever.
    exposeTestHooks = () => {
      (window as unknown as { musaeumE2E: unknown }).musaeumE2E = {
        session: opened.session,
        status: () => opened.status(),
        saved: () => opened.saved(),
        inspect: () => opened.inspect(),
        devicePixelRatio,
        canvas: () => canvas.getBoundingClientRect().toJSON(),
      };
    };
    if (compositorReady) {
      exposeTestHooks();
    }
  }

  if (bench !== null) {
    startMainThreadStress(bench.mainStressMs);
  }
  if (measuringMemory) {
    // A notch of a wheel, as the reader's own input would arrive.
    const scroll = (deltaPx: number): void =>
      opened.send({
        type: 'wheel',
        deltaX: 0,
        deltaY: deltaPx / devicePixelRatio,
        deltaMode: 0,
        ctrlKey: false,
        x: 0,
        y: 0,
        timeMs: performance.now(),
      });
    void runMemoryScenario(scroll, () => opened.report).catch((error) =>
      failBench(bench, String(error)),
    );
  }
}

async function main(): Promise<void> {
  let options: StartupOptions | null = null;
  try {
    options = await invoke<StartupOptions>('startup_options');
    const single = options.bench !== null || options.epub !== null || options.document !== null;
    if (single) {
      document.getElementById('app')?.remove();
      await runDocument(options);
      return;
    }
    for (const id of ['surface', 'reading-bar', 'hud']) {
      document.getElementById(id)?.remove();
    }
    const { mountApp } = await import('./app/main');
    mountApp(options);
  } catch (error) {
    const message = error instanceof Error ? (error.stack ?? error.message) : String(error);
    await failBench(options?.bench ?? null, message);
  }
}

void main();
