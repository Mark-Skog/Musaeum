/**
 * The person's preferences, as the interface applies them: the language of every word, the
 * colors (`data-theme` on the root: none for the system's, `light` or `dark`), and the motion
 * (`data-motion`: `reduce` when the person or the system asks for as little as possible).
 *
 * The host keeps them (`preferences.db`) and says when they change; the system's own settings
 * (dark mode, reduced motion) are followed live while the preference is to follow them.
 */

import { listen } from '@tauri-apps/api/event';
import { useSyncExternalStore } from 'react';

import { api, type PreferencesPatch, type PreferencesView } from '../api';
import { isLocale, setLocale } from '../i18n';

let view: PreferencesView | null = null;
const listeners = new Set<() => void>();
const motionListeners = new Set<(reduced: boolean) => void>();
let lastReduced: boolean | null = null;

const reducedQuery = typeof matchMedia === 'function' ? matchMedia('(prefers-reduced-motion: reduce)') : null;

/** Whether the system asks for as little motion as possible. */
export function systemReducesMotion(): boolean {
  return reducedQuery?.matches ?? false;
}

/** Whether the interface and the readers should move as little as possible now. */
export function isMotionReduced(): boolean {
  return view?.preferences.motion === 'reduce' || systemReducesMotion();
}

function apply(next: PreferencesView | null): void {
  view = next;
  if (next !== null && isLocale(next.preferences.language)) {
    setLocale(next.preferences.language);
  }
  if (typeof document !== 'undefined') {
    const root = document.documentElement;
    const theme = next?.preferences.theme ?? 'system';
    if (theme === 'system') {
      delete root.dataset.theme;
    } else {
      root.dataset.theme = theme;
    }
    root.dataset.motion = isMotionReduced() ? 'reduce' : 'full';
  }
  const reduced = isMotionReduced();
  if (reduced !== lastReduced) {
    lastReduced = reduced;
    for (const listener of motionListeners) {
      listener(reduced);
    }
  }
  for (const listener of listeners) {
    listener();
  }
}

let started: Promise<void> | null = null;

/** Reads the preferences once and applies them; follows the host and the system from then on. */
export function loadPreferences(): Promise<void> {
  started ??= (async () => {
    try {
      apply(await api.preferences());
    } catch {
      // Outside the app (a test page, a benchmark), there are only the defaults.
      apply(null);
    }
    reducedQuery?.addEventListener('change', () => apply(view));
    try {
      await listen<PreferencesView>('preferences-changed', (event) => apply(event.payload));
    } catch {
      // No host to listen to.
    }
  })();
  return started;
}

/** Changes some preferences, applies them, and answers with all of them. */
export async function updatePreferences(patch: PreferencesPatch): Promise<PreferencesView> {
  const next = await api.setPreferences(patch);
  apply(next);
  return next;
}

/** Every preference as it is now, or null before they are read. */
export function currentPreferences(): PreferencesView | null {
  return view;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The preferences, drawing the component again when they change. */
export function usePreferences(): PreferencesView | null {
  return useSyncExternalStore(subscribe, currentPreferences, currentPreferences);
}

/** Calls `listener` whenever whether motion is reduced changes, by the person or the system. */
export function onMotionChange(listener: (reduced: boolean) => void): () => void {
  motionListeners.add(listener);
  return () => motionListeners.delete(listener);
}
