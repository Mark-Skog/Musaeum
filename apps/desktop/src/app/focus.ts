/**
 * Where the focus goes when a panel beside the content closes. A dialog, a menu and a popover are
 * React Aria's, which keep the focus inside while they are open and give it back when they close;
 * a side panel (the details of a book, the reader's contents and annotations) is not modal, so the
 * focus may leave it freely, and when it closes with the focus inside, the focus would fall to the
 * page's body. It goes back to what opened it instead.
 */

import { useEffect, useRef } from 'react';

/** The focus is nowhere a person put it: on the body, or on an element no longer on the page. */
export function focusIsLost(): boolean {
  const active = document.activeElement;
  return active === null || active === document.body || !active.isConnected;
}

/** When `open` turns false with the focus lost, `restore()` puts it back. */
export function useFocusReturn(open: boolean, restore: () => void): void {
  const wasOpen = useRef(open);
  const latest = useRef(restore);
  latest.current = restore;
  useEffect(() => {
    if (wasOpen.current && !open && focusIsLost()) {
      latest.current();
    }
    wasOpen.current = open;
  }, [open]);
}

/** For a panel's `onKeyDown`: Escape, unless something inside already took it, does `then`. */
export function onEscape(then: () => void): (event: React.KeyboardEvent) => void {
  return (event) => {
    if (event.key === 'Escape' && !event.defaultPrevented) {
      event.preventDefault();
      event.stopPropagation();
      then();
    }
  };
}
