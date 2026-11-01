/**
 * The main page's side of a book. It creates the reader's iframe at a separate origin and
 * talks to it only through `messages.ts`. All the page gets from there is untrusted input: a
 * book that broke out of its section could write any of these messages, so what they can do
 * is kept small, and the commands behind them only exist in a benchmark.
 */

import { invoke } from '@tauri-apps/api/core';

import { type FromReader, isFromReader, type ReaderScenario, type ToReader } from './reader/messages';
import type { StartupOptions } from './protocol';

/** What `open_book` returns. */
interface BookInfo {
  origin: string;
  readerUrl: string;
  sizeBytes: number;
  protection: string;
}

interface StrayMessage {
  origin: string;
  type: string;
}

const READER_SANDBOX = 'allow-scripts allow-same-origin';

export async function runBook(options: StartupOptions, fail: (message: string) => void): Promise<void> {
  const openStarted = performance.now();
  const book = await invoke<BookInfo>('open_book', { hostOrigin: location.origin });
  document.body.replaceChildren();

  const frame = document.createElement('iframe');
  frame.setAttribute('sandbox', READER_SANDBOX);
  frame.style.cssText = 'display:block;width:100%;height:100%;border:0';
  frame.src = book.readerUrl;

  const scenario = (options.bench?.scenario ?? 'interactive') as ReaderScenario;
  const strayMessages: StrayMessage[] = [];
  let readerReady: Extract<FromReader, { type: 'ready' }> | null = null;

  const post = (message: ToReader): void => {
    frame.contentWindow?.postMessage(message, book.origin);
  };

  const handle = (message: FromReader): void => {
    switch (message.type) {
      case 'ready':
        readerReady = message;
        post({ type: 'run', scenario });
        break;
      case 'resize-window':
        // Only a benchmark can do this, which the host checks as well.
        void invoke('bench_resize_window', { width: message.width, height: message.height })
          .then(() => post({ type: 'window-resized', id: message.id }))
          .catch((error) => fail(String(error)));
        break;
      case 'phase':
        void invoke('bench_mark_phase', { name: message.name });
        break;
      case 'report':
        void invoke('bench_finish', {
          frontend: JSON.stringify({
            ...message.report,
            book,
            reader: readerReady,
            openToReadyMs: performance.now() - openStarted,
            strayMessages,
            userAgent: navigator.userAgent,
            devicePixelRatio,
          }),
        });
        break;
      case 'failed':
        fail(`reader: ${message.message}`);
        break;
    }
  };

  window.addEventListener('message', (event: MessageEvent<unknown>) => {
    // A section of the book shares the reader's origin, so the origin alone proves nothing.
    // Only the reader's own window counts, and only for messages of the known shapes.
    if (event.source === frame.contentWindow && event.origin === book.origin) {
      if (isFromReader(event.data)) {
        handle(event.data);
      }
      return;
    }
    const data = event.data as { type?: unknown; pwned?: unknown } | null;
    strayMessages.push({
      origin: event.origin,
      type: String(data?.type ?? data?.pwned ?? 'unknown'),
    });
  });

  document.body.append(frame);
}
