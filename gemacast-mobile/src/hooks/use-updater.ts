import { useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { tauriBridge } from '../core/tauri-bridge';
import { useToastStore } from '../stores/toast-store';
import { useUpdateStore } from '../stores/update-store';

/**
 * Hook that drives the auto-update lifecycle.
 *
 * State is stored in a global Zustand store (`useUpdateStore`) so it persists
 * across component mount/unmount cycles (e.g., when the settings drawer is
 * opened and closed).
 *
 * The update check runs once on first mount. Subsequent mounts reuse the
 * existing store state without re-checking.
 */
export function useUpdater() {
  const store = useUpdateStore();

  // --- Actions ---

  const checkForUpdates = useCallback(async () => {
    useUpdateStore.getState().setChecking();
    try {
      await tauriBridge.cleanupStaleUpdates();
    } catch {
      // Non-critical — ignore.
    }

    try {
      const result = await tauriBridge.checkForUpdate();
      if (result) {
        useUpdateStore
          .getState()
          .setAvailable(result.version, result.downloadUrl, result.sha256 ?? null);
      } else {
        useUpdateStore.getState().setUpToDate();
      }
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      useUpdateStore.getState().setError(message);
      useToastStore.getState().show('error', 'Update check failed', message);
    }
  }, []);

  // --- Check for updates on first mount (only if still idle) ---
  useEffect(() => {
    if (store.status !== 'idle') return;
    checkForUpdates();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only on first mount when idle
  }, []);

  // --- Handle app resume (detect return from system installer) ---
  useEffect(() => {
    function onVisibilityChange() {
      if (document.visibilityState === 'visible') {
        useUpdateStore.getState().handleAppResume();
      }
    }

    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => document.removeEventListener('visibilitychange', onVisibilityChange);
  }, []);

  const startDownload = useCallback(async () => {
    const { status, downloadUrl, sha256, version } = useUpdateStore.getState();
    if (status !== 'available' || !downloadUrl || !version) return;

    // Register the progress listener BEFORE starting the download
    // to avoid losing early progress events.
    const unlisten = await listen<number>('update-progress', (event) => {
      const current = useUpdateStore.getState();
      if (current.status === 'downloading') {
        useUpdateStore.getState().setDownloading(event.payload);
      }
    });

    useUpdateStore.getState().setDownloading(0);

    try {
      const apkPath = await tauriBridge.downloadUpdate({
        url: downloadUrl,
        sha256: sha256,
      });
      useUpdateStore.getState().setReady(version, apkPath);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      useUpdateStore.getState().setError(message);
      useToastStore.getState().show('error', 'Download failed', message);
    } finally {
      unlisten();
    }
  }, []);

  const installUpdate = useCallback(async () => {
    const { status, apkPath } = useUpdateStore.getState();
    if (status !== 'ready' || !apkPath) return;

    useUpdateStore.getState().setInstalling();

    try {
      await tauriBridge.installApk({ path: apkPath });
      // The OS installer takes over from here.
      // When the user returns to the app, `handleAppResume` will transition
      // back to 'ready' if the version hasn't changed.
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      useUpdateStore.getState().setError(message);
      useToastStore.getState().show('error', 'Install failed', message);
    }
  }, []);

  const retry = useCallback(() => {
    // Reset to idle so the next mount (or immediate re-run) will re-check.
    useUpdateStore.getState().reset();
  }, []);

  return {
    state: store,
    checkForUpdates,
    startDownload,
    installUpdate,
    retry,
  };
}
