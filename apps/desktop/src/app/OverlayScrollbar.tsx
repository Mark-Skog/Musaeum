/**
 * Reusable React overlay scrollbar and scroll-frame wrapper.
 * Renders a floating right-edge track with zero layout width and binds the imperative controller.
 */

import { type ReactNode, type RefObject, useEffect, useRef } from 'react';

import { isMotionReduced } from './preferences/store';
import { attachOverlayScrollbar } from './scrollbar';

export interface OverlayScrollbarProps {
  /** Direct ref to the scrollable element. */
  targetRef?: RefObject<HTMLElement | null>;
  /** Ancestor host to query `target` inside when the scroll container mounts dynamically. */
  hostRef?: RefObject<HTMLElement | null>;
  target?: string;
  /** Re-binds when the active scrollable view swaps (for example covers vs. table). */
  watchKey?: string;
}

export function OverlayScrollbar({ targetRef, hostRef, target, watchKey }: OverlayScrollbarProps): React.JSX.Element {
  const trackRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const trackEl = trackRef.current;
    const thumbEl = thumbRef.current;
    if (trackEl === null || thumbEl === null) {
      return;
    }

    let detach: (() => void) | null = null;
    let boundEl: HTMLElement | null = null;
    let observer: MutationObserver | null = null;

    const findScrollEl = (): HTMLElement | null => {
      if (targetRef !== undefined) {
        return targetRef.current;
      }
      if (hostRef?.current != null && target !== undefined) {
        return hostRef.current.querySelector<HTMLElement>(target);
      }
      return null;
    };

    const rebind = (): void => {
      const next = findScrollEl();
      if (next === boundEl) {
        return;
      }
      detach?.();
      detach = null;
      boundEl = next;
      if (next !== null) {
        detach = attachOverlayScrollbar(next, trackEl, thumbEl, {
          isReducedMotion: isMotionReduced,
        });
      }
    };

    rebind();

    if (targetRef === undefined && hostRef?.current != null && typeof MutationObserver !== 'undefined') {
      observer = new MutationObserver(rebind);
      observer.observe(hostRef.current, { childList: true, subtree: true });
    }

    return () => {
      observer?.disconnect();
      detach?.();
    };
  }, [targetRef, hostRef, target, watchKey]);

  return (
    <div ref={trackRef} className="overlay-scrollbar" aria-hidden="true">
      <div ref={thumbRef} className="overlay-scrollbar-thumb" />
    </div>
  );
}

/** Positions an overlay scrollbar cleanly over a scrollable container selected inside it. */
export function ScrollFrame({
  target,
  className,
  watchKey,
  children,
}: {
  target: string;
  className?: string;
  watchKey?: string;
  children: ReactNode;
}): React.JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null);
  return (
    <div ref={hostRef} className={className ? `scroll-frame ${className}` : 'scroll-frame'}>
      {children}
      <OverlayScrollbar hostRef={hostRef} target={target} watchKey={watchKey} />
    </div>
  );
}
