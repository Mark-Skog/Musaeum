/**
 * Between the main page and the reader iframe. This is the only channel the two have, and the
 * main page treats whatever comes through it as untrusted: a book that broke out of its
 * section could speak as the reader. So it is small, and nothing in it reaches the app's IPC
 * except what the benchmark needs, which is not there outside a benchmark.
 */

export type ReaderScenario = 'interactive' | 'layout' | 'security' | 'memory' | 'smoke';

export type ToReader =
  | { type: 'run'; scenario: ReaderScenario }
  | { type: 'window-resized'; id: number };

export type FromReader =
  | { type: 'ready'; sections: number; openMs: number }
  | { type: 'resize-window'; id: number; width: number; height: number }
  | { type: 'phase'; name: string }
  | { type: 'report'; report: Record<string, unknown> }
  | { type: 'failed'; message: string };

/**
 * The checks `main.ts` runs on messages from the reader, written as a type guard so nothing
 * unchecked can be used.
 */
export function isFromReader(data: unknown): data is FromReader {
  if (typeof data !== 'object' || data === null) {
    return false;
  }
  const message = data as Record<string, unknown>;
  switch (message.type) {
    case 'ready':
      return typeof message.sections === 'number' && typeof message.openMs === 'number';
    case 'resize-window':
      return (
        Number.isSafeInteger(message.id) &&
        Number.isSafeInteger(message.width) &&
        Number.isSafeInteger(message.height) &&
        (message.width as number) >= 200 &&
        (message.width as number) <= 8192 &&
        (message.height as number) >= 200 &&
        (message.height as number) <= 8192
      );
    case 'phase':
      return typeof message.name === 'string' && message.name.length <= 64;
    case 'report':
      return typeof message.report === 'object' && message.report !== null;
    case 'failed':
      return typeof message.message === 'string';
    default:
      return false;
  }
}
