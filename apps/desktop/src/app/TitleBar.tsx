/**
 * Unified Adobe Acrobat-style desktop title bar and tab strip (`40px`), integrated with Musaeum's
 * light/dark themes while preserving native Win32 frame behaviors (Aero Snap, edge resize, taskbar
 * animations, system menu, and Fitts's Law top-right corner when maximized).
 */

import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { memo, useCallback, useEffect, useRef, useState } from 'react';
import {
  Button,
  Header,
  type Key,
  Menu,
  MenuItem,
  MenuSection,
  MenuTrigger,
  Popover,
  Separator,
} from 'react-aria-components';

import { useT } from './i18n';
import { isMotionReduced } from './preferences/store';
import { type BookTab, HOME_TAB_ID, type TabId } from './tabs-model';

export type TitleBarMenuAction =
  | 'add-folder'
  | 'manage-folders'
  | 'calibre'
  | 'export-jsonld'
  | 'import-jsonld'
  | 'fields'
  | 'preferences';

export interface TitleBarProps {
  /** Open book tabs displayed after the permanent Home (`'home'`) tab. */
  tabs?: readonly BookTab[];
  /** Currently active tab identifier (`'home'` or a `BookTab.id`). */
  activeTabId?: TabId;
  /** Whether a library is currently open (enables library-specific menu actions). */
  libraryOpen?: boolean;
  /** Whether a library-wide annotation export/import is currently running. */
  exchangeBusy?: boolean;
  /** Whether the native window is currently maximized (`IsZoomed`). */
  maximized: boolean;
  /** Hidden while reading in `F11` fullscreen so the book fills the screen without layout jump. */
  hidden?: boolean;
  onSelectTab?(tabId: TabId): void;
  onCloseTab?(tabId: string): void;
  onMenuAction?(action: TitleBarMenuAction): void;
  onOpenPreferences?(): void;
  onMinimize(): void;
  onToggleMaximize(): void;
  onClose(): void;
}

const EMPTY_TABS: readonly BookTab[] = [];

export const TitleBar = memo(function TitleBar({
  tabs = EMPTY_TABS,
  activeTabId = HOME_TAB_ID,
  libraryOpen = true,
  exchangeBusy = false,
  maximized,
  hidden = false,
  onSelectTab,
  onCloseTab,
  onMenuAction,
  onOpenPreferences,
  onMinimize,
  onToggleMaximize,
  onClose,
}: TitleBarProps): React.JSX.Element | null {
  const t = useT();
  const stripRef = useRef<HTMLDivElement>(null);
  const tablistRef = useRef<HTMLDivElement>(null);
  const actionsButtonRef = useRef<HTMLButtonElement>(null);

  // Scroll newly activated book tab smoothly into view inside the horizontal strip.
  useEffect(() => {
    if (hidden || activeTabId === HOME_TAB_ID) {
      return;
    }
    const strip = stripRef.current;
    if (strip === null) {
      return;
    }
    const items = strip.querySelectorAll<HTMLElement>('[data-tab-id]');
    for (const item of items) {
      if (item.getAttribute('data-tab-id') === activeTabId) {
        item.scrollIntoView?.({
          behavior: isMotionReduced() ? 'auto' : 'smooth',
          block: 'nearest',
          inline: 'nearest',
        });
        break;
      }
    }
  }, [activeTabId, tabs.length, hidden]);

  const handleStripWheel = useCallback((event: React.WheelEvent<HTMLDivElement>) => {
    const strip = stripRef.current;
    if (strip === null || strip.scrollWidth <= strip.clientWidth) {
      return;
    }
    if (event.deltaY !== 0 && event.deltaX === 0) {
      strip.scrollLeft += event.deltaY;
    }
  }, []);

  const handleMenuAction = useCallback(
    (key: Key) => {
      const action = String(key) as TitleBarMenuAction;
      if (
        action === 'manage-folders' ||
        action === 'calibre' ||
        action === 'fields' ||
        action === 'preferences'
      ) {
        actionsButtonRef.current?.focus();
      }
      if (onMenuAction !== undefined) {
        onMenuAction(action);
      } else if (action === 'preferences') {
        onOpenPreferences?.();
      }
    },
    [onMenuAction, onOpenPreferences],
  );

  const handleTablistKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const target = event.target as HTMLElement | null;
      if (target?.getAttribute('role') !== 'tab') {
        return;
      }
      const order: TabId[] = [HOME_TAB_ID, ...tabs.map((tab) => tab.id)];
      if (order.length === 0) {
        return;
      }
      const focusedTabId = target.id === 'tab-home' ? HOME_TAB_ID : (target.closest('[data-tab-id]')?.getAttribute('data-tab-id') ?? activeTabId);
      const currentIdx = Math.max(0, order.indexOf(focusedTabId));
      let nextIdx: number | null = null;
      if (event.key === 'ArrowRight') {
        nextIdx = (currentIdx + 1) % order.length;
      } else if (event.key === 'ArrowLeft') {
        nextIdx = (currentIdx - 1 + order.length) % order.length;
      } else if (event.key === 'Home') {
        nextIdx = 0;
      } else if (event.key === 'End') {
        nextIdx = order.length - 1;
      } else if (event.key === 'Delete' && focusedTabId !== HOME_TAB_ID) {
        event.preventDefault();
        onCloseTab?.(focusedTabId);
        return;
      }
      if (nextIdx === null) {
        return;
      }
      event.preventDefault();
      const nextId = order[nextIdx];
      onSelectTab?.(nextId);
      const host = tablistRef.current;
      if (host !== null) {
        const nextBtn =
          nextId === HOME_TAB_ID
            ? host.querySelector<HTMLElement>('#tab-home')
            : host.querySelector<HTMLElement>(`[data-tab-id="${nextId}"] [role="tab"]`);
        nextBtn?.focus();
      }
    },
    [tabs, activeTabId, onSelectTab, onCloseTab],
  );

  if (hidden) {
    return null;
  }

  const actionsLabel = t('library.actions.label');
  const tabsLabel = t('common.tabs.label');
  const homeLabel = t('common.tabs.home');
  const minLabel = t('common.window.minimize');
  const maxLabel = maximized ? t('common.window.restore') : t('common.window.maximize');
  const closeLabel = t('common.window.close');
  const isHomeActive = activeTabId === HOME_TAB_ID;

  const preventFocusSteal = (event: React.MouseEvent): void => {
    event.preventDefault();
  };

  return (
    <div className={maximized ? 'titlebar maximized' : 'titlebar'} role="presentation">
      {!maximized ? <div className="titlebar-resize-top" data-tauri-drag-region="" aria-hidden="true" /> : null}
      <div className="titlebar-left">
        <MenuTrigger>
          <Button
            ref={actionsButtonRef}
            id="library-actions"
            className="titlebar-prefs-button"
            aria-label={actionsLabel}
          >
            <span className="icon-box" title={actionsLabel}>
              <svg className="icon titlebar-gear-icon" viewBox="0 0 24 24" width={15} height={15} fill="none" stroke="currentColor" aria-hidden="true">
                <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.77 1.02 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
              </svg>
            </span>
          </Button>
          <Popover className="popover menu-popover" placement="bottom start">
            <Menu className="menu" aria-label={actionsLabel} onAction={handleMenuAction}>
              <MenuItem id="add-folder" className="menu-item" isDisabled={!libraryOpen}>
                {t('library.addFolder')}
              </MenuItem>
              <MenuItem id="manage-folders" className="menu-item" isDisabled={!libraryOpen}>
                {t('sidebar.folders.manage')}
              </MenuItem>
              <MenuItem id="calibre" className="menu-item" isDisabled={!libraryOpen}>
                {t('library.calibre')}
              </MenuItem>
              <Separator className="menu-separator" />
              <MenuSection>
                <Header className="menu-section-header">{t('library.annotations.label')}</Header>
                <MenuItem id="export-jsonld" className="menu-item" data-action="export-jsonld" isDisabled={!libraryOpen || exchangeBusy}>
                  {t('library.annotations.export')}
                </MenuItem>
                <MenuItem id="import-jsonld" className="menu-item" data-action="import-jsonld" isDisabled={!libraryOpen || exchangeBusy}>
                  {t('library.annotations.import')}
                </MenuItem>
              </MenuSection>
              <Separator className="menu-separator" />
              <MenuItem id="fields" className="menu-item" isDisabled={!libraryOpen}>
                {t('sidebar.fields')}
              </MenuItem>
              <Separator className="menu-separator" />
              <MenuItem id="preferences" className="menu-item" data-action="preferences">
                {t('prefs.open')}
              </MenuItem>
            </Menu>
          </Popover>
        </MenuTrigger>
      </div>
      <div
        ref={tablistRef}
        className="titlebar-tablist"
        role="tablist"
        aria-label={tabsLabel}
        aria-orientation="horizontal"
        onKeyDown={handleTablistKeyDown}
      >
        <button
          type="button"
          id="tab-home"
          role="tab"
          aria-selected={isHomeActive}
          aria-label={homeLabel}
          title={homeLabel}
          tabIndex={isHomeActive ? 0 : -1}
          className={isHomeActive ? 'titlebar-tab titlebar-tab-home active' : 'titlebar-tab titlebar-tab-home'}
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={() => onSelectTab?.(HOME_TAB_ID)}
        >
          <svg className="titlebar-home-icon" viewBox="0 0 16 16" width={16} height={16} fill="none" stroke="currentColor" aria-hidden="true">
            <path d="M1.5 7.5 8 2l6.5 5.5" />
            <path d="M3.25 6.25v7.25a.75.75 0 0 0 .75.75h2.5V9.75h3v4.5H12a.75.75 0 0 0 .75-.75V6.25" />
          </svg>
        </button>
        <div
          ref={stripRef}
          className="titlebar-tab-strip"
          role="presentation"
          onWheel={handleStripWheel}
        >
          {tabs.map((tab) => {
            const isActive = activeTabId === tab.id;
            const closeTabLabel = t('common.tabs.closeTab', { title: tab.title });
            return (
              <div
                key={tab.id}
                data-tab-id={tab.id}
                title={tab.title}
                role="presentation"
                className={isActive ? 'titlebar-tab titlebar-book-tab active' : 'titlebar-tab titlebar-book-tab'}
                onMouseDown={(event) => {
                  if (event.button === 1) {
                    event.preventDefault();
                  }
                }}
                onAuxClick={(event) => {
                  if (event.button === 1) {
                    event.preventDefault();
                    event.stopPropagation();
                    onCloseTab?.(tab.id);
                  }
                }}
              >
                <button
                  type="button"
                  role="tab"
                  aria-selected={isActive}
                  title={tab.title}
                  tabIndex={isActive ? 0 : -1}
                  className="titlebar-tab-trigger"
                  onMouseDown={preventFocusSteal}
                  onClick={() => onSelectTab?.(tab.id)}
                >
                  <span className="titlebar-tab-title">{tab.title}</span>
                </button>
                <button
                  type="button"
                  className="titlebar-tab-close"
                  aria-label={closeTabLabel}
                  title={closeTabLabel}
                  tabIndex={-1}
                  onMouseDown={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                  }}
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab?.(tab.id);
                  }}
                >
                  <svg className="titlebar-tab-close-glyph" viewBox="0 0 10 10" width={9} height={9} fill="none" stroke="currentColor" aria-hidden="true">
                    <path d="M2 2l6 6M8 2l-6 6" />
                  </svg>
                </button>
              </div>
            );
          })}
        </div>
      </div>
      <div className="titlebar-drag" data-tauri-drag-region="" onDoubleClick={onToggleMaximize} />
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

function hasTauriInvoke(): boolean {
  if (!hasTauriWindow()) {
    return false;
  }
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__;
  return typeof internals?.invoke === 'function';
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
    if (hasTauriInvoke()) {
      void invoke('set_window_title', { title }).catch(() => {});
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
