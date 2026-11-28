/**
 * Between the main page and the reader iframe. This is the only channel the two have, and the
 * main page treats whatever comes through it as untrusted: a book that broke out of its
 * section could speak as the reader. So it is small, every message is checked field by field,
 * and nothing in it reaches the app's IPC except the reader's position, which the host parses
 * again, and what the benchmark needs, which is not there outside a benchmark.
 */

import type { FlowMark } from './annotations';
import { isReadingStyle, type ReadingStyle } from './typography';

export type ReaderScenario = 'interactive' | 'layout' | 'security' | 'memory' | 'smoke';

/** The two ways of reading a flowing book. */
export type FlowMode = 'paginated' | 'continuous';

/** One part of the reading order, as the host describes it from the normalized book. */
export interface SectionPlan {
  href: string;
  document: number;
  part: number;
  parts: number;
  bytes: number;
  textStart: number;
  textLength: number;
  bookTextStart: number;
  linear: boolean;
  /** Where the part starts in its document: even CFI steps below `<body>`; empty for the first. */
  start: number[];
}

/** Where to go. */
export type ReaderTarget = { href: string } | { cfi: string } | { fraction: number };

export type ToReader =
  | { type: 'run'; scenario: ReaderScenario }
  | { type: 'window-resized'; id: number }
  | {
      type: 'open';
      mode: FlowMode;
      animated: boolean;
      style: ReadingStyle;
      position: string | null;
      sections: SectionPlan[];
      totalText: number;
      reducedMotion: boolean;
      /** A section's frame title, with `{number}` where its number goes, in the person's language. */
      sectionTitle: string;
    }
  | { type: 'set-mode'; mode: FlowMode }
  | { type: 'set-animation'; animated: boolean }
  | { type: 'set-reduced-motion'; on: boolean }
  /** The language changed: the frames' titles, as in `open`. */
  | { type: 'set-section-title'; template: string }
  | { type: 'set-style'; style: ReadingStyle }
  | { type: 'go-to'; target: ReaderTarget }
  | { type: 'turn'; step: 1 | -1 }
  | { type: 'key'; key: string; shift: boolean }
  /** Every mark to draw, replacing the ones before. */
  | { type: 'marks'; marks: FlowMark[] }
  | { type: 'clear-selection' };

/** A box in CSS pixels of the reader's page. */
export interface ReaderRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Keys the reader acts on itself, for when the host has them. */
export const NAVIGATION_KEYS = new Set([
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'PageUp',
  'PageDown',
  'Home',
  'End',
  ' ',
]);

/** Keys the reader passes up, because what they do belongs to the app. */
export const SHORTCUT_KEYS = [
  'mode',
  'animation',
  'escape',
  'contents',
  'fullscreen',
  'preferences',
  'next-tab',
  'prev-tab',
  'close-tab',
  'tab-1',
  'tab-2',
  'tab-3',
  'tab-4',
  'tab-5',
  'tab-6',
  'tab-7',
  'tab-8',
  'tab-9',
] as const;

export type Shortcut = (typeof SHORTCUT_KEYS)[number];

const ALLOWED_SHORTCUTS: ReadonlySet<string> = new Set<string>(SHORTCUT_KEYS);

export type FromReader =
  | { type: 'ready'; sections: number; openMs: number }
  | { type: 'resize-window'; id: number; width: number; height: number }
  | { type: 'phase'; name: string }
  | { type: 'report'; report: Record<string, unknown> }
  | { type: 'failed'; message: string }
  | { type: 'opened'; openMs: number }
  | {
      type: 'relocate';
      /** The reader's CFI of the anchor: the first character the reader sees. */
      cfi: string;
      section: number;
      /** How far through the book, by text, as the reader can tell; the host gets the exact one. */
      progress: number;
      atStart: boolean;
      atEnd: boolean;
      reason: string;
    }
  | { type: 'layout'; ms: number; change: string }
  | { type: 'shortcut'; key: Shortcut }
  | { type: 'link'; href: string }
  /** What the reader selected, from where to where, or nothing (`start` null). */
  | { type: 'selection'; start: string | null; end: string | null; text: string; rect: ReaderRect | null }
  /** A click on a mark. */
  | { type: 'mark-click'; id: string; rect: ReaderRect };

const MAX_CFI = 4096;
const MAX_HREF = 2048;
const MAX_SELECTION_TEXT = 65536;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

function isCfi(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_CFI && value.startsWith('epubcfi(');
}

function isRect(value: unknown): value is ReaderRect {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const rect = value as Record<string, unknown>;
  return ['x', 'y', 'width', 'height'].every(
    (key) => typeof rect[key] === 'number' && Number.isFinite(rect[key]) && Math.abs(rect[key] as number) < 1e6,
  );
}

function isFraction(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1;
}

/**
 * The checks the main page runs on messages from the reader, written as a type guard so nothing
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
    case 'opened':
      return typeof message.openMs === 'number';
    case 'relocate':
      return (
        typeof message.cfi === 'string' &&
        message.cfi.length <= MAX_CFI &&
        message.cfi.startsWith('epubcfi(') &&
        Number.isSafeInteger(message.section) &&
        (message.section as number) >= 0 &&
        isFraction(message.progress) &&
        typeof message.atStart === 'boolean' &&
        typeof message.atEnd === 'boolean' &&
        typeof message.reason === 'string' &&
        message.reason.length <= 32
      );
    case 'layout':
      return (
        typeof message.ms === 'number' &&
        typeof message.change === 'string' &&
        message.change.length <= 32
      );
    case 'shortcut':
      return typeof message.key === 'string' && ALLOWED_SHORTCUTS.has(message.key);
    case 'link':
      return (
        typeof message.href === 'string' &&
        message.href.length <= MAX_HREF &&
        /^(https?:|mailto:|tel:)/i.test(message.href)
      );
    case 'selection':
      if (message.start === null) {
        return message.end === null && typeof message.text === 'string' && message.rect === null;
      }
      return (
        isCfi(message.start) &&
        isCfi(message.end) &&
        typeof message.text === 'string' &&
        message.text.length <= MAX_SELECTION_TEXT &&
        (message.rect === null || isRect(message.rect))
      );
    case 'mark-click':
      return typeof message.id === 'string' && UUID.test(message.id) && isRect(message.rect);
    default:
      return false;
  }
}

/** The checks the reader runs on what the main page sends. The page is trusted; this catches bugs. */
export function isToReader(data: unknown): data is ToReader {
  if (typeof data !== 'object' || data === null) {
    return false;
  }
  const message = data as Record<string, unknown>;
  switch (message.type) {
    case 'run':
    case 'window-resized':
      return true;
    case 'open':
      return (
        (message.mode === 'paginated' || message.mode === 'continuous') &&
        typeof message.animated === 'boolean' &&
        isReadingStyle(message.style) &&
        (message.position === null || typeof message.position === 'string') &&
        Array.isArray(message.sections)
      );
    case 'set-mode':
      return message.mode === 'paginated' || message.mode === 'continuous';
    case 'set-animation':
      return typeof message.animated === 'boolean';
    case 'set-style':
      return isReadingStyle(message.style);
    case 'go-to':
      return typeof message.target === 'object' && message.target !== null;
    case 'turn':
      return message.step === 1 || message.step === -1;
    case 'key':
      return typeof message.key === 'string' && typeof message.shift === 'boolean';
    case 'marks':
      return Array.isArray(message.marks);
    case 'clear-selection':
      return true;
    default:
      return false;
  }
}
