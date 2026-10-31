import { describe, expect, it, vi } from 'vitest';

import { SharedBufferInbox, type WebviewApi, type WebviewSharedBufferEvent } from './shared-buffer-inbox';

const FIRST = 0x1111_1111;
const SECOND = 0x2222_2222;

function bufferWith(magic: number): ArrayBuffer {
  const buffer = new ArrayBuffer(16);
  new Uint32Array(buffer)[0] = magic;
  return buffer;
}

function fakeWebview(): { api: WebviewApi; post: (buffer: ArrayBuffer) => void } {
  let listener: ((event: WebviewSharedBufferEvent) => void) | undefined;
  return {
    api: {
      addEventListener: (_type, added) => {
        listener = added;
      },
    },
    post: (buffer) => listener?.({ getBuffer: () => buffer } as WebviewSharedBufferEvent),
  };
}

describe('SharedBufferInbox', () => {
  it('hands over a buffer that arrived before anyone asked for it', async () => {
    const { api, post } = fakeWebview();
    const inbox = new SharedBufferInbox(api);
    const buffer = bufferWith(FIRST);
    post(buffer);
    await expect(inbox.next(FIRST, 100)).resolves.toBe(buffer);
  });

  it('wakes a waiter when its buffer arrives', async () => {
    const { api, post } = fakeWebview();
    const inbox = new SharedBufferInbox(api);
    const waiting = inbox.next(FIRST, 1000);
    const buffer = bufferWith(FIRST);
    post(buffer);
    await expect(waiting).resolves.toBe(buffer);
  });

  it('tells buffers apart by their magic number, in any order', async () => {
    const { api, post } = fakeWebview();
    const inbox = new SharedBufferInbox(api);
    const [first, second] = [bufferWith(FIRST), bufferWith(SECOND)];
    const waitingForSecond = inbox.next(SECOND, 1000);
    post(first);
    post(second);
    await expect(waitingForSecond).resolves.toBe(second);
    await expect(inbox.next(FIRST, 100)).resolves.toBe(first);
  });

  it('keeps buffers with the same magic number in arrival order', async () => {
    const { api, post } = fakeWebview();
    const inbox = new SharedBufferInbox(api);
    const [one, two] = [bufferWith(FIRST), bufferWith(FIRST)];
    post(one);
    post(two);
    await expect(inbox.next(FIRST, 100)).resolves.toBe(one);
    await expect(inbox.next(FIRST, 100)).resolves.toBe(two);
  });

  it('gives up when a promised buffer never arrives, and stays usable', async () => {
    vi.useFakeTimers();
    try {
      const { api, post } = fakeWebview();
      const inbox = new SharedBufferInbox(api);
      const waiting = inbox.next(FIRST, 50);
      const failed = expect(waiting).rejects.toThrow('never posted');
      await vi.advanceTimersByTimeAsync(51);
      await failed;

      const buffer = bufferWith(FIRST);
      post(buffer);
      await expect(inbox.next(FIRST, 50)).resolves.toBe(buffer);
    } finally {
      vi.useRealTimers();
    }
  });
});
