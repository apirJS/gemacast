import { useState, useEffect, useCallback, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { tauriBridge } from '../core/tauri-bridge';
import { useToastStore } from '../stores/toast-store';

export type UpdateState =
  | { status: 'idle' }
  | { status: 'checking' }
  | { status: 'available'; version: string; downloadUrl: string }
  | { status: 'downloading'; percent: number }
  | { status: 'ready'; version: string; apkPath: string }
  | { status: 'installing' }
  | { status: 'error'; message: string }
  | { status: 'up-to-date' };

export function useUpdater() {
  const [state, setState] = useState<UpdateState>({ status: 'idle' });
  const stateRef = useRef(state);

  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  // Check for updates on mount (app launch).
  useEffect(() => {
    let cancelled = false;

    async function check() {
      setState({ status: 'checking' });
      try {
        const result = await tauriBridge.checkForUpdate();
        if (cancelled) return;
        if (result) {
          setState({
            status: 'available',
            version: result.version,
            downloadUrl: result.downloadUrl,
          });
        } else {
          setState({ status: 'up-to-date' });
        }
      } catch (e) {
        if (cancelled) return;
        const message = e instanceof Error ? e.message : String(e);
        setState({ status: 'error', message });
        useToastStore.getState().show('warning', `Update check failed: ${message}`);
      }
    }

    check();
    return () => {
      cancelled = true;
    };
  }, []);

  const startDownload = useCallback(async () => {
    const current = stateRef.current;
    if (current.status !== 'available') return;

    const { version, downloadUrl } = current;

    // Register the progress listener BEFORE starting the download
    // to avoid losing early progress events.
    const unlisten = await listen<number>('update-progress', (event) => {
      setState((prev) => {
        if (prev.status !== 'downloading') return prev;
        return { ...prev, percent: event.payload };
      });
    });

    setState({ status: 'downloading', percent: 0 });

    try {
      const apkPath = await tauriBridge.downloadUpdate({ url: downloadUrl });
      setState({ status: 'ready', version, apkPath });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setState({ status: 'error', message });
      useToastStore.getState().show('error', `Download failed: ${message}`);
    } finally {
      unlisten();
    }
  }, []);

  const installUpdate = useCallback(async () => {
    const current = stateRef.current;
    if (current.status !== 'ready') return;

    const { apkPath } = current;
    setState({ status: 'installing' });

    try {
      await tauriBridge.installApk({ path: apkPath });
      // The OS installer takes over from here — we stay in 'installing' state.
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setState({ status: 'error', message });
      useToastStore.getState().show('error', `Install failed: ${message}`);
    }
  }, []);

  return { state, startDownload, installUpdate };
}
