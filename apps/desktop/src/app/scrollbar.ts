/**
 * Lightweight custom overlay scrollbar engine with exponential fade and FPS-independent
 * exponential damping on drag/track click, plus reusable exponential damping math.
 */

export const SCROLL_HIDE_DELAY_MS = 900;
export const SCROLL_DAMPING_K = 20;
export const WHEEL_DAMPING_K = 18;
export const WHEEL_LINE_PX = 40;
export const WHEEL_PAGE_FACTOR = 0.85;
export const SCROLL_SETTLE_EPSILON = 0.5;
export const MAX_FRAME_DT = 0.05;
export const MIN_THUMB_HEIGHT = 28;

const CANCEL_KEYS = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ']);

export function normalizeWheelDelta(deltaY: number, deltaMode: number, clientHeight: number): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) {
    return 0;
  }
  if (deltaMode === 1) {
    return deltaY * WHEEL_LINE_PX;
  }
  if (deltaMode === 2) {
    return deltaY * Math.max(1, clientHeight) * WHEEL_PAGE_FACTOR;
  }
  return deltaY;
}

export function canConsumeVerticalWheel(scrollTop: number, scrollHeight: number, clientHeight: number, deltaPx: number): boolean {
  const maxScroll = Math.max(0, scrollHeight - clientHeight);
  if (maxScroll <= 0 || !Number.isFinite(deltaPx) || deltaPx === 0) {
    return false;
  }
  if (deltaPx < 0) {
    return scrollTop > 0.5;
  }
  return scrollTop < maxScroll - 0.5;
}

export interface ThumbMetrics {
  visible: boolean;
  thumbHeight: number;
  thumbOffset: number;
  maxScroll: number;
  maxOffset: number;
}

/**
 * Exponential damping step independent of refresh rate.
 * Snaps to `target` once within `epsilon` so animations reach exact rest and stop `rAF`.
 */
export function expDamp(
  current: number,
  target: number,
  dtSeconds: number,
  k = SCROLL_DAMPING_K,
  epsilon = SCROLL_SETTLE_EPSILON,
): number {
  if (Math.abs(target - current) < epsilon) {
    return target;
  }
  const clampedDt = Math.max(0, Math.min(dtSeconds, MAX_FRAME_DT));
  const alpha = 1 - Math.exp(-k * clampedDt);
  const next = current + (target - current) * alpha;
  return Math.abs(target - next) < epsilon ? target : next;
}

/** Computes overlay thumb dimensions and offset for a scroll container. */
export function computeThumbMetrics(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  trackHeight: number,
  minThumbHeight = MIN_THUMB_HEIGHT,
): ThumbMetrics {
  if (clientHeight <= 0 || trackHeight <= 0 || scrollHeight <= clientHeight + 1) {
    return { visible: false, thumbHeight: 0, thumbOffset: 0, maxScroll: 0, maxOffset: 0 };
  }
  const maxScroll = Math.max(0, scrollHeight - clientHeight);
  const rawHeight = (clientHeight / scrollHeight) * trackHeight;
  const thumbHeight = Math.min(trackHeight, Math.max(minThumbHeight, rawHeight));
  const maxOffset = Math.max(0, trackHeight - thumbHeight);
  const clampedScroll = Math.min(maxScroll, Math.max(0, scrollTop));
  const thumbOffset = maxScroll > 0 && maxOffset > 0 ? (clampedScroll / maxScroll) * maxOffset : 0;
  return { visible: true, thumbHeight, thumbOffset, maxScroll, maxOffset };
}

/** Maps a thumb offset inside the track back to `scrollTop`. */
export function thumbOffsetToScrollTop(offset: number, maxOffset: number, maxScroll: number): number {
  if (maxOffset <= 0 || maxScroll <= 0) {
    return 0;
  }
  const clamped = Math.min(maxOffset, Math.max(0, offset));
  return (clamped / maxOffset) * maxScroll;
}

/** Keys that scroll a container natively and must cancel any active thumb/track glide. */
export function isScrollCancelKey(key: string): boolean {
  return CANCEL_KEYS.has(key);
}

export interface OverlayScrollbarOptions {
  delayMs?: number;
  k?: number;
  isReducedMotion?: () => boolean;
}

/**
 * Attaches overlay scrollbar behavior to `scrollEl`, driving `trackEl` and `thumbEl`.
 * Returns a cleanup function that removes all listeners, timers, observers, and `rAF`.
 */
export function attachOverlayScrollbar(
  scrollEl: HTMLElement,
  trackEl: HTMLElement,
  thumbEl: HTMLElement,
  options: OverlayScrollbarOptions = {},
): () => void {
  const delayMs = options.delayMs ?? SCROLL_HIDE_DELAY_MS;
  const dampingK = options.k ?? SCROLL_DAMPING_K;
  const isReducedMotion = options.isReducedMotion ?? (() => false);

  let hasOverflow = false;
  let pointerInZone = false;
  let dragging = false;
  let scrollActive = false;
  let targetScrollTop = scrollEl.scrollTop;
  let animSource: 'drag' | 'wheel' | null = null;
  let activeDampingK = dampingK;
  let rafId: number | null = null;
  let lastFrameTime = 0;
  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  let grabOffsetY = 0;
  let dragCleanup: (() => void) | null = null;
  let disposed = false;

  const updateVisibility = (): void => {
    const show = hasOverflow && (pointerInZone || dragging || scrollActive);
    trackEl.classList.toggle('visible', show);
  };

  const clearHideTimer = (): void => {
    if (hideTimer !== null) {
      clearTimeout(hideTimer);
      hideTimer = null;
    }
  };

  const scheduleHide = (): void => {
    clearHideTimer();
    hideTimer = setTimeout(() => {
      hideTimer = null;
      scrollActive = false;
      updateVisibility();
    }, delayMs);
  };

  const syncMetrics = (): ThumbMetrics => {
    const metrics = computeThumbMetrics(
      scrollEl.scrollTop,
      scrollEl.scrollHeight,
      scrollEl.clientHeight,
      trackEl.clientHeight,
    );
    hasOverflow = metrics.visible;
    trackEl.classList.toggle('has-overflow', hasOverflow);
    if (hasOverflow) {
      thumbEl.style.height = `${metrics.thumbHeight}px`;
      thumbEl.style.setProperty('--thumb-y', `${metrics.thumbOffset}px`);
    } else {
      thumbEl.style.height = '0px';
      thumbEl.style.setProperty('--thumb-y', '0px');
    }
    updateVisibility();
    return metrics;
  };

  const cancelAnimation = (): void => {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    animSource = null;
    lastFrameTime = 0;
  };

  const stepAnimation = (now: number): void => {
    rafId = null;
    if (disposed) {
      return;
    }
    const maxScroll = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight);
    const clampedTarget = Math.min(maxScroll, Math.max(0, targetScrollTop));
    const current = scrollEl.scrollTop;
    const dt = lastFrameTime === 0 ? 1 / 60 : (now - lastFrameTime) / 1000;
    lastFrameTime = now;

    const next = expDamp(current, clampedTarget, dt, activeDampingK);
    scrollEl.scrollTop = next;
    syncMetrics();

    if (Math.abs(clampedTarget - next) >= SCROLL_SETTLE_EPSILON) {
      rafId = requestAnimationFrame(stepAnimation);
    } else {
      animSource = null;
      lastFrameTime = 0;
    }
  };

  const startAnimation = (target: number, source: 'drag' | 'wheel' = 'drag'): void => {
    const maxScroll = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight);
    const clamped = Math.min(maxScroll, Math.max(0, target));
    targetScrollTop = clamped;

    if (isReducedMotion()) {
      cancelAnimation();
      scrollEl.scrollTop = clamped;
      syncMetrics();
      return;
    }

    if (Math.abs(clamped - scrollEl.scrollTop) < SCROLL_SETTLE_EPSILON) {
      scrollEl.scrollTop = clamped;
      syncMetrics();
      cancelAnimation();
      return;
    }

    animSource = source;
    activeDampingK = source === 'wheel' ? WHEEL_DAMPING_K : dampingK;
    if (rafId === null) {
      lastFrameTime = 0;
      rafId = requestAnimationFrame(stepAnimation);
    }
  };

  const markScrollActivity = (): void => {
    if (!hasOverflow) {
      syncMetrics();
    }
    if (!hasOverflow) {
      return;
    }
    scrollActive = true;
    updateVisibility();
    if (!pointerInZone && !dragging) {
      scheduleHide();
    } else {
      clearHideTimer();
    }
  };

  const onScroll = (): void => {
    syncMetrics();
    markScrollActivity();
  };

  const onWheel = (event: WheelEvent): void => {
    if (event.ctrlKey || event.metaKey) {
      cancelAnimation();
      syncMetrics();
      markScrollActivity();
      return;
    }
    if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
      return;
    }
    if (dragging) {
      cancelAnimation();
      syncMetrics();
      markScrollActivity();
      return;
    }
    const deltaPx = normalizeWheelDelta(event.deltaY, event.deltaMode, scrollEl.clientHeight);
    if (deltaPx === 0 || isReducedMotion()) {
      syncMetrics();
      markScrollActivity();
      return;
    }
    const baseTop = animSource === 'wheel' ? targetScrollTop : scrollEl.scrollTop;
    const canConsumeTarget = canConsumeVerticalWheel(baseTop, scrollEl.scrollHeight, scrollEl.clientHeight, deltaPx);
    const canConsumeCurrent = canConsumeVerticalWheel(scrollEl.scrollTop, scrollEl.scrollHeight, scrollEl.clientHeight, deltaPx);
    if (!canConsumeTarget && !canConsumeCurrent) {
      syncMetrics();
      markScrollActivity();
      return;
    }
    event.preventDefault();
    startAnimation(baseTop + deltaPx, 'wheel');
    markScrollActivity();
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    if (isScrollCancelKey(event.key)) {
      cancelAnimation();
      markScrollActivity();
    }
  };

  const onPointerEnter = (): void => {
    pointerInZone = true;
    clearHideTimer();
    syncMetrics();
  };

  const onPointerLeave = (): void => {
    pointerInZone = false;
    if (!dragging && scrollActive) {
      scheduleHide();
    } else {
      updateVisibility();
    }
  };

  const onPointerDown = (event: PointerEvent): void => {
    if (event.button !== 0) {
      return;
    }
    const metrics = syncMetrics();
    if (!metrics.visible) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();

    dragCleanup?.();
    cancelAnimation();
    dragging = true;
    clearHideTimer();
    trackEl.classList.add('dragging');
    updateVisibility();

    const trackRect = trackEl.getBoundingClientRect();
    const targetNode = event.target as Node | null;
    const hitThumb = targetNode !== null && (targetNode === thumbEl || thumbEl.contains(targetNode));
    if (hitThumb) {
      const thumbRect = thumbEl.getBoundingClientRect();
      grabOffsetY = event.clientY - thumbRect.top;
    } else {
      grabOffsetY = metrics.thumbHeight / 2;
      const desiredOffset = event.clientY - trackRect.top - grabOffsetY;
      startAnimation(thumbOffsetToScrollTop(desiredOffset, metrics.maxOffset, metrics.maxScroll));
    }

    const onPointerMove = (moveEvent: PointerEvent): void => {
      const fresh = computeThumbMetrics(
        scrollEl.scrollTop,
        scrollEl.scrollHeight,
        scrollEl.clientHeight,
        trackEl.clientHeight,
      );
      if (!fresh.visible) {
        return;
      }
      const freshTrack = trackEl.getBoundingClientRect();
      const desiredOffset = moveEvent.clientY - freshTrack.top - grabOffsetY;
      startAnimation(thumbOffsetToScrollTop(desiredOffset, fresh.maxOffset, fresh.maxScroll));
    };

    const finishDrag = (): void => {
      cleanupDrag();
      dragging = false;
      trackEl.classList.remove('dragging');
      if (!pointerInZone && scrollActive) {
        scheduleHide();
      } else {
        updateVisibility();
      }
    };

    const win: EventTarget = trackEl.ownerDocument?.defaultView ?? (typeof window !== 'undefined' ? window : trackEl);

    const cleanupDrag = (): void => {
      win.removeEventListener('pointermove', onPointerMove as EventListener);
      win.removeEventListener('pointerup', finishDrag);
      win.removeEventListener('pointercancel', finishDrag);
      dragCleanup = null;
    };

    dragCleanup = cleanupDrag;
    win.addEventListener('pointermove', onPointerMove as EventListener);
    win.addEventListener('pointerup', finishDrag);
    win.addEventListener('pointercancel', finishDrag);
  };

  scrollEl.addEventListener('scroll', onScroll, { passive: true });
  scrollEl.addEventListener('wheel', onWheel, { passive: false });
  scrollEl.addEventListener('keydown', onKeyDown);
  trackEl.addEventListener('pointerenter', onPointerEnter);
  trackEl.addEventListener('pointerleave', onPointerLeave);
  trackEl.addEventListener('pointerdown', onPointerDown);

  let resizeObserver: ResizeObserver | null = null;
  let observedChild: Element | null = null;
  const syncChildObservation = (): void => {
    if (resizeObserver === null) {
      return;
    }
    const nextChild = scrollEl.firstElementChild;
    if (nextChild !== observedChild) {
      if (observedChild !== null) {
        resizeObserver.unobserve(observedChild);
      }
      if (nextChild !== null) {
        resizeObserver.observe(nextChild);
      }
      observedChild = nextChild;
    }
  };

  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => {
      syncMetrics();
    });
    resizeObserver.observe(scrollEl);
    resizeObserver.observe(trackEl);
    syncChildObservation();
  }

  let mutationObserver: MutationObserver | null = null;
  if (typeof MutationObserver !== 'undefined') {
    mutationObserver = new MutationObserver(() => {
      syncChildObservation();
      syncMetrics();
    });
    mutationObserver.observe(scrollEl, { childList: true });
  }

  syncMetrics();

  return () => {
    disposed = true;
    dragCleanup?.();
    cancelAnimation();
    clearHideTimer();
    scrollEl.removeEventListener('scroll', onScroll);
    scrollEl.removeEventListener('wheel', onWheel);
    scrollEl.removeEventListener('keydown', onKeyDown);
    trackEl.removeEventListener('pointerenter', onPointerEnter);
    trackEl.removeEventListener('pointerleave', onPointerLeave);
    trackEl.removeEventListener('pointerdown', onPointerDown);
    resizeObserver?.disconnect();
    mutationObserver?.disconnect();
    trackEl.classList.remove('visible', 'has-overflow', 'dragging');
  };
}
