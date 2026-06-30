import { useEffect, useRef } from 'react';

/**
 * Manages the Screen Wake Lock API to prevent the display from dimming.
 *
 * When `enabled` is true, requests a screen wake lock. When false (or on
 * unmount), releases it. Automatically re-acquires the lock when the page
 * regains visibility (standard Web Wake Lock pattern).
 *
 * Graceful no-op if the browser/WebView does not support `navigator.wakeLock`.
 */
export function useWakeLock(enabled: boolean) {
  const sentinelRef = useRef<WakeLockSentinel | null>(null);

  useEffect(() => {
    if (!enabled || !('wakeLock' in navigator)) return;

    let cancelled = false;

    const acquire = async () => {
      try {
        if (!cancelled && document.visibilityState === 'visible') {
          const sentinel = await navigator.wakeLock.request('screen');
          if (cancelled) {
            sentinel.release().catch(() => {});
          } else {
            sentinelRef.current = sentinel;
            sentinel.addEventListener('release', () => {
              sentinelRef.current = null;
            });
          }
        }
      } catch {
        // Wake lock request can fail (e.g. low battery, permission denied).
        // Silently ignore — this is a best-effort feature.
      }
    };

    const handleVisibilityChange = () => {
      if (document.visibilityState === 'visible' && !sentinelRef.current) {
        acquire();
      }
    };

    acquire();
    document.addEventListener('visibilitychange', handleVisibilityChange);

    return () => {
      cancelled = true;
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      sentinelRef.current?.release().catch(() => {});
      sentinelRef.current = null;
    };
  }, [enabled]);
}
