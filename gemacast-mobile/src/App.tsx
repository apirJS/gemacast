import { useEffect, useState } from 'react';
import { getDeviceInfo, type DeviceInfoResponse } from 'tauri-plugin-device-info-api';
import { useAppStore } from './stores/app-store';
import { tauriBridge } from './core/tauri-bridge';
import { getOrCreateDeviceId } from './core/persistence';
import { useTauriEvents } from './hooks/use-tauri-events';
import { useNetworkMonitor } from './hooks/use-network-monitor';
import { startListening } from './hooks/use-discovery';
import { AppShell } from './components/layout/AppShell';

function AppInner() {
  useTauriEvents();
  useNetworkMonitor();

  useEffect(() => {
    const mode = useAppStore.getState().settings.mode;
    startListening(mode);

    // Hardware back button double-press to exit logic
    window.history.pushState({ root: true }, '', '#root');
    let lastBackPressed = 0;

    const handlePopState = () => {
      if (window.location.hash === '') {
        const now = Date.now();
        if (now - lastBackPressed < 2000) {
          import('@tauri-apps/api/window').then((m) => m.getCurrentWindow().close()).catch(console.warn);
        } else {
          lastBackPressed = now;
          // useToastStore.getState().show is required, need to import it
          import('./stores/toast-store').then((m) => m.useToastStore.getState().show('info', 'Press back again to exit'));
          window.history.pushState({ root: true }, '', '#root');
        }
      }
    };

    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, []);

  return <AppShell />;
}

export function App() {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    (async () => {
      let bestName = 'Unknown Android Device';
      let finalUuid = getOrCreateDeviceId();
      let localIp = '127.0.0.1';

      try {
        const rawInfo: DeviceInfoResponse = await getDeviceInfo();
        if (rawInfo.device_name) bestName = rawInfo.device_name;
        else if (rawInfo.manufacturer && rawInfo.model) {
          bestName = `${rawInfo.manufacturer} ${rawInfo.model}`;
        }
        finalUuid = rawInfo.uuid || rawInfo.android_id || finalUuid;
      } catch (e) {
        console.warn('Failed to fetch device info:', e);
      }

      try {
        localIp = await tauriBridge.getLocalIp();
      } catch (e) {
        console.warn('Failed to fetch local IP:', e);
      }

      useAppStore.getState().init({
        deviceId: finalUuid,
        deviceName: bestName,
        ip: localIp,
      });

      try {
        const modes = await tauriBridge.getConnectionStatus();
        useAppStore.getState().setAvailableModes(modes);
      } catch (e) {
        console.warn('Failed to fetch initial connection status:', e);
      }

      const theme = useAppStore.getState().settings.theme;
      document.documentElement.classList.toggle('dark', theme === 'dark');
      document.documentElement.classList.toggle('light', theme === 'light');

      setReady(true);
    })();
  }, []);

  if (!ready) {
    return (
      <div className="flex min-h-dvh items-center justify-center">
        <div className="h-8 w-8 animate-spin rounded-full border-2 border-primary border-t-transparent" />
      </div>
    );
  }

  return <AppInner />;
}
