import { useEffect, useRef } from 'react';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { disconnect, killPlayback } from './use-connection';
import { startListening, stopListening } from './use-discovery';
import { tauriBridge } from '../core/tauri-bridge';
import { ConnectionMode, Status } from '../core/types';
import { saveLastSender } from '../core/persistence';

export function useNetworkMonitor() {
  const networkIdRef = useRef('');
  const lastModeRef = useRef<ConnectionMode | null>(null);

  useEffect(() => {
    const store = useAppStore;

    const checkNetwork = async () => {
      try {
        const netState = await tauriBridge.getNetworkState();
        const localIp = netState.localIp;
        const networkId = netState.networkId;
        const modes = netState.modes;

        const currentState = store.getState();

        if (
          currentState.settings.mode === ConnectionMode.Wifi &&
          (currentState.deviceInfo.ip !== localIp ||
            networkIdRef.current !== networkId)
        ) {
          networkIdRef.current = networkId;

          if (
            currentState.status === Status.Playing ||
            currentState.status === Status.Connected ||
            currentState.status === Status.Paused
          ) {
            try {
              await disconnect(true);
            } catch (e) {
              console.warn('[NetworkMonitor] Graceful disconnect on network hop failed:', e);
            }
          }

          killPlayback();
          saveLastSender(null);

          store.getState().dismissError();
          store.getState().patch({
            deviceInfo: { ...currentState.deviceInfo, ip: localIp },
            discoveredSenders: [],
            connectedSender: null,
            lastConnectedSender: null,
            status: Status.Listening,
          });

          await stopListening();
          startListening(currentState.settings.mode);
        }

        store.getState().setAvailableModes(modes);

        const currentMode = currentState.settings.mode;

        if (
          currentMode === ConnectionMode.Usb &&
          !modes.usb &&
          (currentState.status === Status.Playing ||
          currentState.status === Status.Paused)
        ) {
          disconnect(true);
          killPlayback();
        } else if (
          currentMode === ConnectionMode.Wifi &&
          !modes.wifi &&
          (currentState.status === Status.Playing ||
          currentState.status === Status.Paused)
        ) {
          disconnect(true);
          killPlayback();
        }
      } catch {
        // Ignore network errors during check
      }
    };

    const handleOnline = () => {
      const state = store.getState();
      if (state.settings.mode === ConnectionMode.Wifi) {
        store.getState().dismissError();
        store.getState().patch({ isNetworkAvailable: true });
        useToastStore.getState().show('info', 'Network online');
      }
      checkNetwork();
    };

    const handleOffline = () => {
      const state = store.getState();
      if (state.settings.mode !== ConnectionMode.Wifi) {
        return;
      }

      store.getState().patch({
        isNetworkAvailable: false,
        connectionHealth: 'lost',
        discoveredSenders: [],
      });
      useToastStore.getState().show('warning', 'Network offline');

      if (state.connectedSender || state.status === Status.Playing || state.status === Status.Paused) {
        store.getState().patch({
          status: Status.Listening,
          connectedSender: null,
        });
        store.getState().resetLatency();
      }
    };

    checkNetwork();
    const interval = setInterval(checkNetwork, 3000);
    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const nav = navigator as any;
    const connection = nav.connection || nav.mozConnection || nav.webkitConnection;
    if (connection) {
      connection.addEventListener('change', checkNetwork);
    }

    return () => {
      clearInterval(interval);
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
      if (connection) {
        connection.removeEventListener('change', checkNetwork);
      }
    };
  }, []);

  useEffect(() => {
    const unsub = useAppStore.subscribe((state) => {
      const currentMode = state.settings.mode;
      if (lastModeRef.current !== null && currentMode !== lastModeRef.current) {
        lastModeRef.current = currentMode; // Update immediately to prevent infinite loop
        (async () => {
          if (
            state.status === Status.Connected ||
            state.status === Status.Playing ||
            state.status === Status.Paused ||
            state.status === Status.Reconnecting
          ) {
            try {
              await disconnect(true);
              killPlayback();
            } catch (e) {
              console.warn('[NetworkMonitor] Cleanup during mode switch failed:', e);
            }
          }

          useAppStore.getState().dismissError();
          useAppStore.getState().patch({
            discoveredSenders: [],
            status: Status.Listening,
          });

          await stopListening();
          startListening(currentMode);
        })();
      } else {
        lastModeRef.current = currentMode;
      }
    });
    return unsub;
  }, []);
}
