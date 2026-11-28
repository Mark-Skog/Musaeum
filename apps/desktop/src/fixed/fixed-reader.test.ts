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
  initialPage: number | null;
  initialZoom: number | null;
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
              initialPage: raw.initialState?.page ?? null,
              initialZoom: raw.initialState?.zoom ?? null,
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
    expect(inits).toEqual([{ sessionId: 1, ringTag: 1, slabTag: 1, initialPage: null, initialZoom: null }]);
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
    expect(inits).toEqual([{ sessionId: 2, ringTag: 2, slabTag: 2, initialPage: null, initialZoom: null }]);

    await secondReader.close();
    expect(closeCalls).toEqual([1, 2]);
  });

  it('aborts intermediate tabs on rapid Tab A -> Tab B -> Tab C -> Home -> Tab A stress switching with <= 1 active session', async () => {
    let nextSessionId = 1;
    let activeSessions = 0;
    let peakActiveSessions = 0;
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
        activeSessions++;
        peakActiveSessions = Math.max(peakActiveSessions, activeSessions);
        postSharedBuffer(makeBuffer(RING_MAGIC, id));
        postSharedBuffer(makeBuffer(SLAB_MAGIC, id));
        await new Promise((resolve) => setTimeout(resolve, 25));
        return makeSession(id);
      }
      if (cmd === 'close_session') {
        const closedId = (args?.session as number | null | undefined) ?? null;
        closeCalls.push(closedId);
        if (closedId !== null && activeSessions > 0) {
          activeSessions--;
        }
        return;
      }
      throw new Error(`Unexpected command: ${cmd}`);
    });

    const { FixedReader } = await import('./fixed-reader');
    const options = makeOptions();

    const abortA1 = new AbortController();
    const abortB = new AbortController();
    const abortC = new AbortController();

    // Start Tab A, let it enter open_session, then rapidly switch -> Tab B -> Tab C -> Home
    const openA1 = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'tab-a.pdf',
      documentId: 101,
      shortcuts: false,
      bench: null,
      signal: abortA1.signal,
    });
    await new Promise((resolve) => setTimeout(resolve, 5));

    abortA1.abort();
    const openB = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'tab-b.pdf',
      documentId: 102,
      shortcuts: false,
      bench: null,
      signal: abortB.signal,
    });
    abortB.abort();
    const openC = FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'tab-c.pdf',
      documentId: 103,
      shortcuts: false,
      bench: null,
      signal: abortC.signal,
    });
    // Switch to Home: abort C as well
    abortC.abort();

    await expect(openA1).rejects.toMatchObject({ name: 'AbortError' });
    await expect(openB).rejects.toMatchObject({ name: 'AbortError' });
    await expect(openC).rejects.toMatchObject({ name: 'AbortError' });

    // On Home: 0 active sessions, and queued intermediate tabs B and C never even called open_session!
    expect(activeSessions).toBe(0);
    expect(peakActiveSessions).toBe(1);
    expect(openCalls).toEqual(['tab-a.pdf']);
    expect(closeCalls).toEqual([1]);

    // Return to Tab A with its saved snapshot: opens cleanly at exact page & zoom
    const readerA2 = await FixedReader.open({
      options,
      canvas: makeCanvas(),
      document: 'tab-a.pdf',
      documentId: 101,
      initialState: {
        page: 42,
        u: 0.15,
        v: 0.68,
        progress: 0.1,
        mode: 'continuous',
        pageTurnAnimation: false,
        zoom: 1.75,
        spreads: 'double',
      },
      shortcuts: false,
      bench: null,
    });
    await readerA2.ready();

    expect(activeSessions).toBe(1);
    expect(peakActiveSessions).toBe(1);
    expect(openCalls).toEqual(['tab-a.pdf', 'tab-a.pdf']);
    expect(inits).toEqual([
      { sessionId: 2, ringTag: 2, slabTag: 2, initialPage: 42, initialZoom: 1.75 },
    ]);

    // Suspend back to Home: session closes deterministically -> 0 active sessions
    await readerA2.close();
    expect(activeSessions).toBe(0);
    expect(closeCalls).toEqual([1, 2]);
  });
});

describe('TileRenderer explicit WebGL2 GPU resource cleanup on tab suspension', () => {
  it('deletes textures, buffers, VAOs, programs and framebuffers, zeroes OffscreenCanvas, and loses context without spurious restore', async () => {
    const deletedTextures: unknown[] = [];
    const deletedBuffers: unknown[] = [];
    const deletedVaos: unknown[] = [];
    const deletedPrograms: unknown[] = [];
    const deletedShaders: unknown[] = [];
    const deletedFramebuffers: unknown[] = [];
    let loseContextCalls = 0;
    let nextHandle = 1;

    const canvasListeners = new Map<string, Set<(ev: Event) => void>>();
    const loseContextExt = {
      loseContext: () => {
        loseContextCalls++;
        // Simulate browser firing webglcontextlost + webglcontextrestored if listeners remained attached
        let prevented = false;
        const lostEvent = {
          preventDefault() {
            prevented = true;
          },
        } as unknown as Event;
        for (const cb of [...(canvasListeners.get('webglcontextlost') ?? [])]) {
          cb(lostEvent);
        }
        if (prevented) {
          for (const cb of [...(canvasListeners.get('webglcontextrestored') ?? [])]) {
            cb({} as Event);
          }
        }
      },
    };

    const mockGl = {
      MAX_ARRAY_TEXTURE_LAYERS: 0x88ff,
      RENDERER: 0x1f01,
      VERTEX_SHADER: 0x8b31,
      FRAGMENT_SHADER: 0x8b30,
      COMPILE_STATUS: 0x8b81,
      LINK_STATUS: 0x8b82,
      TEXTURE0: 0x84c0,
      TEXTURE1: 0x84c1,
      TEXTURE_2D: 0x0de1,
      TEXTURE_2D_ARRAY: 0x8c1a,
      RGBA8: 0x8058,
      TEXTURE_MIN_FILTER: 0x2801,
      TEXTURE_MAG_FILTER: 0x2800,
      TEXTURE_WRAP_S: 0x2802,
      TEXTURE_WRAP_T: 0x2803,
      TEXTURE_MAX_LEVEL: 0x813d,
      LINEAR: 0x2601,
      LINEAR_MIPMAP_NEAREST: 0x2701,
      CLAMP_TO_EDGE: 0x812f,
      UNPACK_ALIGNMENT: 0x0cf5,
      ARRAY_BUFFER: 0x8892,
      DYNAMIC_DRAW: 0x88e8,
      FLOAT: 0x1406,
      FRAMEBUFFER: 0x8d40,
      COLOR_ATTACHMENT0: 0x8ce0,
      DEPTH_TEST: 0x0b71,
      BLEND: 0x0be2,
      getParameter: (param: number) => (param === 0x88ff ? 256 : 'Mock GPU'),
      getExtension: (name: string) => (name === 'WEBGL_lose_context' ? loseContextExt : null),
      createShader: () => ({ kind: 'shader', id: nextHandle++ }),
      shaderSource: () => {},
      compileShader: () => {},
      getShaderParameter: () => true,
      getShaderInfoLog: () => '',
      deleteShader: (s: unknown) => deletedShaders.push(s),
      createProgram: () => ({ kind: 'program', id: nextHandle++ }),
      attachShader: () => {},
      detachShader: () => {},
      linkProgram: () => {},
      getProgramParameter: () => true,
      getProgramInfoLog: () => '',
      useProgram: () => {},
      deleteProgram: (p: unknown) => deletedPrograms.push(p),
      getUniformLocation: () => ({ kind: 'uniform', id: nextHandle++ }),
      uniform1i: () => {},
      uniform1f: () => {},
      uniform2f: () => {},
      uniform3fv: () => {},
      createTexture: () => ({ kind: 'texture', id: nextHandle++ }),
      activeTexture: () => {},
      bindTexture: () => {},
      texStorage3D: () => {},
      texStorage2D: () => {},
      texParameteri: () => {},
      pixelStorei: () => {},
      deleteTexture: (t: unknown) => deletedTextures.push(t),
      createBuffer: () => ({ kind: 'buffer', id: nextHandle++ }),
      bindBuffer: () => {},
      bufferData: () => {},
      deleteBuffer: (b: unknown) => deletedBuffers.push(b),
      createVertexArray: () => ({ kind: 'vao', id: nextHandle++ }),
      bindVertexArray: () => {},
      enableVertexAttribArray: () => {},
      vertexAttribPointer: () => {},
      vertexAttribDivisor: () => {},
      deleteVertexArray: (v: unknown) => deletedVaos.push(v),
      createFramebuffer: () => ({ kind: 'fbo', id: nextHandle++ }),
      bindFramebuffer: () => {},
      framebufferTexture2D: () => {},
      deleteFramebuffer: (f: unknown) => deletedFramebuffers.push(f),
      disable: () => {},
      clearColor: () => {},
      viewport: () => {},
    } as unknown as WebGL2RenderingContext;

    const fakeOffscreen = {
      width: 1280,
      height: 800,
      getContext: () => mockGl,
      addEventListener: (type: string, cb: (ev: Event) => void, opts?: AddEventListenerOptions) => {
        let set = canvasListeners.get(type);
        if (!set) {
          set = new Set();
          canvasListeners.set(type, set);
        }
        set.add(cb);
        opts?.signal?.addEventListener('abort', () => set?.delete(cb), { once: true });
      },
    } as unknown as OffscreenCanvas;

    const { TileRenderer } = await import('../compositor/renderer');
    let lostCount = 0;
    let restoredCount = 0;
    const renderer = new TileRenderer(
      fakeOffscreen,
      64 * 1024 * 1024,
      () => lostCount++,
      () => restoredCount++,
    );

    // 4 shaders compiled across 2 programs, immediately detached & deleted after linking
    expect(deletedShaders).toHaveLength(4);

    // Destroy deterministically releases every GPU allocation and loses the WebGL2 context once
    renderer.destroy();
    renderer.destroy(); // Idempotent

    expect(deletedTextures).toHaveLength(2); // tile 2D_ARRAY + frame 2D texture
    expect(deletedFramebuffers).toHaveLength(1);
    expect(deletedBuffers).toHaveLength(2); // tileBuffer + markBuffer
    expect(deletedVaos).toHaveLength(2); // tileVertexArray + markVertexArray
    expect(deletedPrograms).toHaveLength(2); // tileProgram + markProgram
    expect(fakeOffscreen.width).toBe(0);
    expect(fakeOffscreen.height).toBe(0);
    expect(loseContextCalls).toBe(1);
    expect(lostCount).toBe(0);
    expect(restoredCount).toBe(0);
  });
});
