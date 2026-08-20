import { useEffect } from 'react';
import { BellOff, Settings } from 'lucide-react';
import { tauriBridge } from '../../core/tauri-bridge';
import { useAppStore } from '../../stores/app-store';
import { useToastStore } from '../../stores/toast-store';

/**
 * Shown only when the app cannot post its streaming notification.
 *
 * `notRequired` covers both "Android is older than 13" and "the state could not
 * be read", so it renders nothing — a notice that appears when nothing is
 * actually wrong is worse than no notice at all.
 */
export function NotificationPermissionNotice() {
  const permission = useAppStore((s) => s.notificationPermission);
  const off = permission === 'denied' || permission === 'blocked';

  // Fixing this means leaving the app, so the state probed at startup is stale
  // by the time the user comes back. Re-read it on return instead of making them
  // restart to clear a notice they have already acted on.
  useEffect(() => {
    if (!off) return;
    const refresh = () => {
      if (document.visibilityState !== 'visible') return;
      void tauriBridge
        .getNotificationPermission()
        .then((next) => useAppStore.getState().setNotificationPermission(next))
        .catch((error) => console.warn('Failed to re-read notification permission:', error));
    };
    document.addEventListener('visibilitychange', refresh);
    return () => document.removeEventListener('visibilitychange', refresh);
  }, [off]);

  if (!off) return null;

  const openSettings = () => {
    void tauriBridge.openNotificationSettings().catch((error) => {
      useToastStore.getState().show('error', 'Could not open notification settings', String(error));
    });
  };

  return (
    <div className="mb-4 rounded-xl border border-border bg-accent/30 p-4">
      <div className="flex items-start gap-3">
        <BellOff className="mt-0.5 h-4 w-4 shrink-0 text-status-warn" />
        <div className="min-w-0 flex-1 space-y-1">
          <p className="text-[0.9rem] font-semibold text-foreground">Notifications are off</p>
          <p className="text-[0.8rem] text-muted-foreground">
            Streaming still works. What you lose are the Pause and Disconnect buttons outside the
            app, so you have to open Gemacast to stop a stream.
          </p>
          <p className="text-[0.8rem] text-muted-foreground">
            {permission === 'blocked'
              ? 'Android will not ask again, so turn notifications on in system settings.'
              : 'Gemacast asks again the next time you open it, or turn notifications on in system settings now.'}
          </p>
        </div>
      </div>
      <button
        type="button"
        className="mt-3 flex items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-[0.8rem] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground active:bg-accent/80"
        onClick={openSettings}
      >
        <Settings className="h-3.5 w-3.5" />
        Open settings
      </button>
    </div>
  );
}
