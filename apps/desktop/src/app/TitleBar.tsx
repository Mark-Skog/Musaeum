/**
 * Custom desktop title bar integrated with Musaeum's light and dark themes while keeping native
 * Win32 window frame behaviors (Aero Snap, edge resize, taskbar animations, system menu).
 */

import { getCurrentWindow } from '@tauri-apps/api/window';
import { memo, useCallback, useEffect, useRef, useState } from 'react';

import { useT } from './i18n';

export interface TitleBarProps {
  /** Dynamic window title (`Musaeum` in the library, or the active book's title). */
  title: string;
  /** Whether the native window is currently maximized (`IsZoomed`). */
  maximized: boolean;
  /** Hidden while reading in `F11` fullscreen so the book fills the screen without layout jump. */
  hidden?: boolean;
  onMinimize(): void;
  onToggleMaximize(): void;
  onClose(): void;
}

export const TitleBar = memo(function TitleBar({
  title,
  maximized,
  hidden = false,
  onMinimize,
  onToggleMaximize,
  onClose,
}: TitleBarProps): React.JSX.Element | null {
  const t = useT();

  if (hidden) {
    return null;
  }

  const minLabel = t('common.window.minimize');
  const maxLabel = maximized ? t('common.window.restore') : t('common.window.maximize');
  const closeLabel = t('common.window.close');
  const preventFocusSteal = (event: React.MouseEvent): void => {
    event.preventDefault();
  };

  return (
    <div className={maximized ? 'titlebar maximized' : 'titlebar'} role="presentation">
      {!maximized ? <div className="titlebar-resize-top" data-tauri-drag-region="" aria-hidden="true" /> : null}
      <div className="titlebar-drag" data-tauri-drag-region="" onDoubleClick={onToggleMaximize}>
        <span className="titlebar-seal" aria-hidden="true">
          <svg viewBox="0 0 32 32" width={16} height={16} fill="none">
            <rect className="titlebar-seal-plate" x={1} y={1} width={30} height={30} rx={6} />
            <path className="titlebar-seal-spine" d="M8.5 6.6h2.9v18.8H8.5z" />
            <path className="titlebar-seal-cover" d="M13.4 6.6h4.15v8.6l1.5-1.27 1.5 1.27V6.6H23.5v18.8H13.4z" />
          </svg>
        </span>
        <span className="titlebar-title" title={title}>
          {title}
        </span>
      </div>
      <div className="titlebar-controls">
        <button
          type="button"
          id="titlebar-minimize"
          className="titlebar-button"
          aria-label={minLabel}
          title={minLabel}
          onMouseDown={preventFocusSteal}
          onClick={onMinimize}
        >
          <svg className="titlebar-glyph" viewBox="0 0 10 10" width={10} height={10} fill="none" stroke="currentColor" aria-hidden="true">
            <line x1={1} y1={5.5} x2={9} y2={5.5} />
          </svg>
        </button>
        <button
          type="button"
          id="titlebar-maximize"
          className="titlebar-button"
          aria-label={maxLabel}
          title={maxLabel}
          onMouseDown={preventFocusSteal}
          onClick={onToggleMaximize}
        >
          {maximized ? (
            <svg className="titlebar-glyph" viewBox="0 0 10 10" width={10} height={10} fill="none" stroke="currentColor" aria-hidden="true">
              <rect x={1.5} y={3.5} width={5.5} height={5.5} />
              <path d="M3.5 3.5V1.5h5v5H6.5" />
            </svg>
          ) : (
            <svg className="titlebar-glyph" viewBox="0 0 10 10" width={10} height={10} fill="none" stroke="currentColor" aria-hidden="true">
              <rect x={1.5} y={1.5} width={7} height={7} />
            </svg>
          )}
        </button>
        <button
          type="button"
          id="titlebar-close"
          className="titlebar-button close"
          aria-label={closeLabel}
          title={closeLabel}
          onMouseDown={preventFocusSteal}
          onClick={onClose}
        >
          <svg className="titlebar-glyph" viewBox="0 0 10 10" width={10} height={10} fill="none" stroke="currentColor" aria-hidden="true">
            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" />
          </svg>
        </button>
      </div>
    </div>
  );
});

export interface WindowChrome {
  maximized: boolean;
  fullscreen: boolean;
  minimize(): void;
  toggleMaximize(): void;
  close(): void;
  setFullscreen(next: boolean): Promise<void>;
  toggleFullscreen(): Promise<void>;
}

function hasTauriWindow(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in (window as unknown as Record<string, unknown>);
}

/** Synchronizes custom title bar state and actions with the native Tauri window. */
export function useWindowChrome(title: string): WindowChrome {
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreenState] = useState(false);
  const fullscreenRef = useRef(false);
  fullscreenRef.current = fullscreen;

  useEffect(() => {
    if (typeof document !== 'undefined') {
      document.title = title;
    }
  }, [title]);

  const syncWindowState = useCallback(async () => {
    if (!hasTauriWindow()) {
      return;
    }
    try {
      const win = getCurrentWindow();
      const [isMax, isFull] = await Promise.all([win.isMaximized(), win.isFullscreen()]);
      setMaximized(isMax);
      const domFull = typeof document !== 'undefined' && document.fullscreenElement != null;
      setFullscreenState(isFull || domFull);
    } catch {
      // Ignore during teardown or non-window test harnesses.
    }
  }, []);

  useEffect(() => {
    if (!hasTauriWindow()) {
      return;
    }
    let active = true;
    let unlistenResize: (() => void) | null = null;

    void syncWindowState();

    const win = getCurrentWindow();
    void win
      .onResized(() => {
        if (active) {
          void syncWindowState();
        }
      })
      .then((stop) => {
        if (active) {
          unlistenResize = stop;
        } else {
          stop();
        }
      })
      .catch(() => {});

    const onDomResize = (): void => {
      void syncWindowState();
    };
    window.addEventListener('resize', onDomResize);
    document.addEventListener('fullscreenchange', onDomResize);

    return () => {
      active = false;
      unlistenResize?.();
      window.removeEventListener('resize', onDomResize);
      document.removeEventListener('fullscreenchange', onDomResize);
    };
  }, [syncWindowState]);

  const minimize = useCallback(() => {
    if (!hasTauriWindow()) {
      return;
    }
    void getCurrentWindow()
      .minimize()
      .catch(() => {});
  }, []);

  const toggleMaximize = useCallback(() => {
    if (!hasTauriWindow()) {
      return;
    }
    void getCurrentWindow()
      .toggleMaximize()
      .then(() => syncWindowState())
      .catch(() => {});
  }, [syncWindowState]);

  const close = useCallback(() => {
    if (!hasTauriWindow()) {
      return;
    }
    void getCurrentWindow()
      .close()
      .catch(() => {});
  }, []);

  const setFullscreen = useCallback(async (next: boolean) => {
    if (!hasTauriWindow()) {
      setFullscreenState(next);
      return;
    }
    try {
      await getCurrentWindow().setFullscreen(next);
      setFullscreenState(next);
      const isMax = await getCurrentWindow().isMaximized();
      setMaximized(isMax);
    } catch {
      // Ignore if unavailable.
    }
  }, []);

  const toggleFullscreen = useCallback(async () => {
    await setFullscreen(!fullscreenRef.current);
  }, [setFullscreen]);

  return {
    maximized,
    fullscreen,
    minimize,
    toggleMaximize,
    close,
    setFullscreen,
    toggleFullscreen,
  };
}
