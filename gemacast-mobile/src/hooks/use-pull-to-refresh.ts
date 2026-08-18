import { useEffect, useRef, useState } from 'react';

/**
 * Marks a subtree the pull gesture must never claim.
 *
 * Touch events bubble, so a listener on the scroll container also sees drags
 * that started in any descendant — including a popover that merely *renders*
 * inside the list but scrolls independently of it. Without this, scrolling the
 * process picker rubber-banded the sender list behind it and kicked off a
 * discovery refresh.
 *
 * Exported so the marker and this check cannot drift apart.
 */
export const PULL_REFRESH_IGNORE_ATTR = 'data-pull-refresh-ignore';

/** True when the touch landed inside a subtree marked with the ignore attribute. */
function startedInIgnoredRegion(target: EventTarget | null): boolean {
  // Touch targets can be text nodes, which have no `closest`.
  const element =
    target instanceof Element ? target : target instanceof Node ? target.parentElement : null;
  return element?.closest(`[${PULL_REFRESH_IGNORE_ATTR}]`) != null;
}

type PullToRefreshOptions = {
  /** Runs when the user releases past the threshold. May be async. */
  onRefresh: () => void | Promise<void>;
  /** Pull distance (px) required to trigger a refresh. */
  threshold?: number;
  /** Hard cap on the rubber-band travel (px). */
  maxPull?: number;
  /** Drag-to-pull divisor; higher feels stiffer. */
  resistance?: number;
  /** Keep the spinner up at least this long so a fast rescan is still perceptible. */
  minSpinMs?: number;
  /** When true, the gesture is inert (native scroll only). */
  disabled?: boolean;
};

type PullToRefreshState<T extends HTMLElement> = {
  /** Attach to the scrollable element the gesture reads `scrollTop` from. */
  ref: React.RefObject<T | null>;
  /** Current rubber-band distance in px (0 when idle). */
  pull: number;
  /** True while `onRefresh` (plus the min-spin hold) is in flight. */
  refreshing: boolean;
};

/**
 * Touch-driven pull-to-refresh for a scroll container.
 *
 * Only owns the gesture while the scroller is pinned at the top and the finger
 * moves down, so ordinary scrolling is untouched. The `touchmove` listener is
 * registered non-passive precisely so it can `preventDefault()` the native
 * overscroll while rubber-banding — React's synthetic `onTouchMove` is passive
 * and cannot, which is why this wires listeners imperatively.
 */
export function usePullToRefresh<T extends HTMLElement>({
  onRefresh,
  threshold = 64,
  maxPull = 96,
  resistance = 2,
  minSpinMs = 600,
  disabled = false,
}: PullToRefreshOptions): PullToRefreshState<T> {
  const ref = useRef<T | null>(null);
  const [pull, setPull] = useState(0);
  const [refreshing, setRefreshing] = useState(false);

  // Mirror live values into refs so the imperative listeners never read stale
  // closures and the effect need not re-subscribe on every drag frame.
  const startY = useRef<number | null>(null);
  const owning = useRef(false);
  const pullRef = useRef(0);
  const refreshingRef = useRef(false);
  const onRefreshRef = useRef(onRefresh);
  useEffect(() => {
    onRefreshRef.current = onRefresh;
  }, [onRefresh]);

  useEffect(() => {
    const el = ref.current;
    if (!el || disabled) return;

    let holdTimer: ReturnType<typeof setTimeout> | null = null;

    const setPullValue = (value: number) => {
      pullRef.current = value;
      setPull(value);
    };

    const reset = () => {
      startY.current = null;
      owning.current = false;
      setPullValue(0);
    };

    const onTouchStart = (event: TouchEvent) => {
      // Leaving startY null is what disarms the gesture: onTouchMove and
      // onTouchEnd both bail on it, so the nested region scrolls natively and
      // never gets a preventDefault from us.
      if (refreshingRef.current || el.scrollTop > 0 || startedInIgnoredRegion(event.target)) {
        startY.current = null;
        return;
      }
      startY.current = event.touches?.[0]?.clientY ?? null;
      owning.current = false;
    };

    const onTouchMove = (event: TouchEvent) => {
      if (startY.current === null || refreshingRef.current) return;
      const currentY = event.touches?.[0]?.clientY;
      if (currentY === undefined) return;

      const delta = currentY - startY.current;
      // Finger moving up, or the list scrolled off the top: hand back to native.
      if (delta <= 0 || el.scrollTop > 0) {
        if (owning.current) {
          owning.current = false;
          setPullValue(0);
        }
        return;
      }

      owning.current = true;
      // Suppress the browser's own overscroll while we drive the rubber-band.
      if (event.cancelable) event.preventDefault();
      setPullValue(Math.min(maxPull, delta / resistance));
    };

    const finishHold = () => {
      refreshingRef.current = false;
      setRefreshing(false);
      setPullValue(0);
    };

    const onTouchEnd = () => {
      if (startY.current === null) return;
      const shouldRefresh = owning.current && pullRef.current >= threshold;
      startY.current = null;
      owning.current = false;

      if (!shouldRefresh) {
        setPullValue(0);
        return;
      }

      refreshingRef.current = true;
      setRefreshing(true);
      setPullValue(threshold);

      const startedAt = Date.now();
      void Promise.resolve()
        .then(() => onRefreshRef.current())
        .catch(() => {})
        .finally(() => {
          const remaining = Math.max(0, minSpinMs - (Date.now() - startedAt));
          holdTimer = setTimeout(finishHold, remaining);
        });
    };

    el.addEventListener('touchstart', onTouchStart, { passive: true });
    el.addEventListener('touchmove', onTouchMove, { passive: false });
    el.addEventListener('touchend', onTouchEnd, { passive: true });
    el.addEventListener('touchcancel', reset, { passive: true });

    return () => {
      if (holdTimer) clearTimeout(holdTimer);
      el.removeEventListener('touchstart', onTouchStart);
      el.removeEventListener('touchmove', onTouchMove);
      el.removeEventListener('touchend', onTouchEnd);
      el.removeEventListener('touchcancel', reset);
    };
  }, [disabled, threshold, maxPull, resistance, minSpinMs]);

  return { ref, pull, refreshing };
}
