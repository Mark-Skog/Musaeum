import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { FromWorker, ProbeRequest, ToWorker } from '../compositor/messages';
import {
  PROBE_HOST_WORD,
  PROBE_MAGIC,
  PROBE_SCRIPT_WORD,
  RING_MAGIC,
  type SessionInfo,
  SLAB_MAGIC,
  type StartupOptions,
} from '../protocol';
import type { WebviewSharedBufferEvent } from '../shared-buffer-inbox';

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
}));

vi.mock('../app/preferences/store', () => ({
  isMotionReduced: () => false,
}));

interface InitSnapshot {
  sessionId: number;
  ringTag: number | null;
  slabTag: number | null;
}

function makeBuffer(magic: number, tag = 0, byteLength = 4096): ArrayBuffer {
  const buf = new ArrayBuffer(byteLength);
  const words = new Uint32Array(buf);
  words[0] = magic;
  words[3] = tag;
  return buf;
}

function makeOptions(): StartupOptions {
  return {
    document: null,
    e2e: false,
    epub: null,
    bookProtection: 'full',
    delivery: 'auto',
    deliveryFaults: [],
    workers: 4,
    pullBatch: 8,
    gpuLayers: null,
    tileBudgetMb: null,
    noSandbox: false,
    width: 1280,
    height: 800,
    bench: null,
  };
}

function makeSession(id: number): SessionInfo {
  return {
    id,
    format: 'pdf',
    delivery: 'c',
    pageCount: 402,
    orientation: 'portrait',
    slotCount: 64,
    sandboxed: true,
    openUs: 1500,
    geometryUs: 200,
    pageLayout: null,
    direction: null,
    geometryCached: true,
    documentId: id,
    readingState: null,
    background: { toClassify: 0, toDraw: 0, toTrace: 0 },
  };
}

function makeCanvas(): HTMLCanvasElement {
  return {
    clientWidth: 800,
    clientHeight: 600,
    style: { cursor: '' },
    addEventListener: vi.fn(),
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 800, height: 600 }),
    transferControlToOffscreen: () => ({ width: 800, height: 600 }) as unknown as OffscreenCanvas,
  } as unknown as HTMLCanvasElement;
}

describe('FixedReader session lifecycle and shared-buffer serialization', () => {
  let postSharedBuffer: (buffer: ArrayBuffer) => void;
  let probeRegions: ArrayBuffer[];
  let inits: InitSnapshot[];
  let terminatedWorkers: number;
  const savedGlobals: Record<string, unknown> = {};

  beforeEach(() => {
    vi.resetModules();
    invokeMock.mockReset();
    probeRegions = [];
    inits = [];
    terminatedWorkers = 0;

    let sharedListener: ((event: WebviewSharedBufferEvent) => void) | undefined;
    postSharedBuffer = (buffer: ArrayBuffer) => {
      sharedListener?.({ getBuffer: () => buffer } as WebviewSharedBufferEvent);
    };

    class FakeWorker {
      private readonly messageListeners = new Set<(event: MessageEvent<FromWorker>) => void>();
      private readonly held = new Map<string, ArrayBuffer>();

      addEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
        if (type === 'message' && typeof listener === 'function') {
          this.messageListeners.add(listener as (event: MessageEvent<FromWorker>) => void);
        }
      }

      removeEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
        if (type === 'message' && typeof listener === 'function') {
          this.messageListeners.delete(listener as (event: MessageEvent<FromWorker>) => void);
        }
      }

      postMessage(raw: ToWorker): void {
        queueMicrotask(() => {
          if (raw.type === 'probe') {
            this.handleProbe(raw);
            return;
          }
          if (raw.type === 'init') {
            inits.push({
              sessionId: raw.session.id,
              ringTag: raw.direct ? new Uint32Array(raw.direct.ring)[3] : null,
              slabTag: raw.direct ? new Uint32Array(raw.direct.slab)[3] : null,
            });
            this.emit({
              type: 'ready',
              gpu: 'mock-webgl2',
              pageCount: raw.session.pageCount,
              clockRttMs: 0.2,
            });
            return;
          }
          if (raw.type === 'flush') {
            this.emit({ type: 'flushed' });
          }
        });
      }

      terminate(): void {
        terminatedWorkers++;
        this.messageListeners.clear();
      }

      private handleProbe(req: ProbeRequest): void {
        if (req.step === 'take') {
          this.held.set(req.label, req.buffer);
          this.emit({
            type: 'probe-reply',
            label: req.label,
            byteLength: req.buffer.byteLength,
            firstWord: PROBE_MAGIC,
            hostWord: 0,
            workerHasWebviewApi: false,
          });
          return;
        }
        const buf = this.held.get(req.label);
        if (req.step === 'read') {
          const hostWord = buf ? Atomics.load(new Uint32Array(buf), PROBE_HOST_WORD) : 0;
          this.emit({
            type: 'probe-reply',
            label: req.label,
            byteLength: buf?.byteLength ?? 0,
            firstWord: PROBE_MAGIC,
            hostWord,
            workerHasWebviewApi: false,
          });
          return;
        }
        if (req.step === 'write') {
          if (buf) {
            Atomics.store(new Uint32Array(buf), PROBE_SCRIPT_WORD, req.value);
          }
          this.emit({
            type: 'probe-reply',
            label: req.label,
            byteLength: buf?.byteLength ?? 0,
            firstWord: PROBE_MAGIC,
            hostWord: 0,
            workerHasWebviewApi: false,
          });
          return;
        }
        if (req.step === 'forget') {
          this.held.delete(req.label);
          this.emit({
            type: 'probe-reply',
            label: req.label,
            byteLength: 0,
            firstWord: 0,
            hostWord: 0,
            workerHasWebviewApi: false,
          });
        }
      }

      private emit(data: FromWorker): void {
        for (const listener of [...this.messageListeners]) {
          listener({ data } as MessageEvent<FromWorker>);
        }
      }
    }

    class FakeResizeObserver {
      observe(): void {}
      disconnect(): void {}
    }

    for (const key of [
      'window',
      'screen',
      'devicePixelRatio',
      'crossOriginIsolated',
      'requestAnimationFrame',
      'Worker',
      'ResizeObserver',
    ]) {
      savedGlobals[key] = (globalThis as Record<string, unknown>)[key];
    }

    Object.assign(globalThis, {
      window: {
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        chrome: {
          webview: {
            addEventListener: (_type: string, cb: (event: WebviewSharedBufferEvent) => void) => {
              sharedListener = cb;
            },
          },
        },
      },
      screen: { width: 1920, height: 1080 },
      devicePixelRatio: 1,
      crossOriginIsolated: true,
      requestAnimationFrame: vi.fn(() => 1),
      Worker: FakeWorker,
      ResizeObserver: FakeResizeObserver,
    });
  });

  afterEach(() => {
    for (const [key, value] of Object.entries(savedGlobals)) {
      if (value === undefined) {
        delete (globalThis as Record<string, unknown>)[key];
      } else {
        (globalThis as Record<string, unknown>)[key] = value;
      }
    }
  });

  function installDefaultHostMock(openDelaysMs: number[] = []): {
    openCalls: string[];
    closeCalls: (number | null)[];
  } {
    let nextSessionId = 1;
    const openCalls: string[] = [];
    const closeCalls: (number | null)[] = [];

    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === 'open_delivery_probe') {
        const ro = makeBuffer(PROBE_MAGIC);
        const rw = makeBuffer(PROBE_MAGIC);
        probeRegions = [ro, rw];
        postSharedBuffer(ro);
        postSharedBuffer(rw);
        return;
      }
      if (cmd === 'probe_exchange') {
        const value = (args?.value as number) >>> 0;
        return probeRegions.map((region) => {
          const words = new Uint32Array(region);
          Atomics.store(words, PROBE_HOST_WORD, value);
          return Atomics.load(words, PROBE_SCRIPT_WORD);
        });
      }
      if (cmd === 'open_session') {
        const req = args?.args as { document: string };
        const id = nextSessionId++;
        openCalls.push(req.document);
        postSharedBuffer(makeBuffer(RING_MAGIC, id));
        postSharedBuffer(makeBuffer(SLAB_MAGIC, id));
        const waitMs = openDelaysMs.shift() ?? 0;
        if (waitMs > 0) {
          await new Promise((resolve) => setTimeout(resolve, waitMs));
        }
        return makeSession(id);
      }
      if (cmd === 'close_session') {
        closeCalls.push((args?.session as number | null | undefined) ?? null);
        return;
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    return { openCalls, closeCalls };
  }

  it('skips host probe and session spawn when aborted synchronously (React StrictMode mount)', async () => {
    const { openCalls, closeCalls } = installDefaultHostMock();
    const { FixedReader } = await import('./fixed-reader');
    const options = makeOptions();

    const firstAbort = new AbortController();
    const secondAbort = new AbortController();

    const firstOpen = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'synthetic-large.pdf',
      shortcuts: false,
      bench: null,
      signal: firstAbort.signal,
    });
    // StrictMode unmounts the first effect synchronously before microtasks flush, then mounts #2.
    firstAbort.abort();
    const secondOpen = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'synthetic-large.pdf',
      shortcuts: false,
      bench: null,
      signal: secondAbort.signal,
    });

    await expect(firstOpen).rejects.toMatchObject({ name: 'AbortError' });
    const reader = await secondOpen;
    await reader.ready();

    expect(openCalls).toEqual(['synthetic-large.pdf']);
    expect(closeCalls).toEqual([]);
    expect(reader.session?.id).toBe(1);
    expect(inits).toEqual([{ sessionId: 1, ringTag: 1, slabTag: 1 }]);
    expect(terminatedWorkers).toBe(1);

    // Idempotent close sends the exact session id once.
    await Promise.all([reader.close(), reader.close()]);
    expect(closeCalls).toEqual([1]);
  });

  it('serializes overlapping opens and drains/closes a slow session aborted mid-startup without buffer mix-up', async () => {
    const { openCalls, closeCalls } = installDefaultHostMock([40, 5]);
    const { FixedReader } = await import('./fixed-reader');
    const options = makeOptions();

    const firstAbort = new AbortController();
    const firstOpen = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'slow-first.pdf',
      shortcuts: false,
      bench: null,
      signal: firstAbort.signal,
    });

    // Let open_session #1 enter its slow worker startup, then abort #1 and immediately start #2.
    await new Promise((resolve) => setTimeout(resolve, 10));
    firstAbort.abort();
    const secondOpen = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'second.pdf',
      shortcuts: false,
      bench: null,
    });

    await expect(firstOpen).rejects.toMatchObject({ name: 'AbortError' });
    const secondReader = await secondOpen;
    await secondReader.ready();

    expect(openCalls).toEqual(['slow-first.pdf', 'second.pdf']);
    // Session 1 was cleanly closed by id before Session 2 opened, and Session 2 got its own buffers.
    expect(closeCalls).toEqual([1]);
    expect(secondReader.session?.id).toBe(2);
    expect(inits).toEqual([{ sessionId: 2, ringTag: 2, slabTag: 2 }]);

    await secondReader.close();
    expect(closeCalls).toEqual([1, 2]);
  });
});
