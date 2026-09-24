import { getDeviceInfo, type DeviceInfoResponse } from 'tauri-plugin-device-info-api';
import { getOrCreateDeviceId } from '../core/persistence';
import { tauriBridge } from '../core/tauri-bridge';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { appEventsController } from './app-events-controller';
import { connectionController } from './connection-controller';
import { discoveryController } from './discovery-controller';
import { networkController } from './network-controller';
import { settingsController } from './settings-controller';
import { updateController } from './update-controller';

const EXIT_CONFIRMATION_WINDOW_MS = 2000;

export class ApplicationController {
  private lastBackPress = 0;

  async initialize(): Promise<void> {
    const deviceInfo = await this.loadDeviceInfo();
    useAppStore.getState().init(deviceInfo);

    const [modes, exclusiveSupported] = await Promise.all([
      tauriBridge.getConnectionStatus().catch((error) => {
        console.warn('Failed to fetch initial connection status:', error);
        return useAppStore.getState().availableModes;
      }),
      tauriBridge.checkExclusiveSupport().catch((error) => {
        console.warn('Failed to probe exclusive mode support:', error);
        return true;
      }),
    ]);
    useAppStore.getState().setAvailableModes(modes);
    useAppStore.getState().setExclusiveSupported(exclusiveSupported);
    settingsController.setTheme(useAppStore.getState().settings.theme);
    await connectionController.reconnectOnAppOpen().catch(console.warn);
  }

  start(): () => void {
    void discoveryController.start(useAppStore.getState().settings.mode);
    const stopEvents = appEventsController.start();
    const stopNetwork = networkController.start();
    const stopUpdater = updateController.start();

    window.history.pushState(null, '', '#root');
    window.addEventListener('popstate', this.handleBackButton);
    document.addEventListener('visibilitychange', this.handleVisibilityChange);

    return () => {
      stopEvents();
      stopNetwork();
      stopUpdater();
      window.removeEventListener('popstate', this.handleBackButton);
      document.removeEventListener('visibilitychange', this.handleVisibilityChange);
    };
  }

  private async loadDeviceInfo() {
    let deviceName = 'Unknown Android Device';
    let deviceId = getOrCreateDeviceId();
    let ip = '127.0.0.1';

    try {
      const info: DeviceInfoResponse = await getDeviceInfo();
      deviceName =
        info.device_name ||
        (info.manufacturer && info.model ? `${info.manufacturer} ${info.model}` : deviceName);
      deviceId = info.uuid || info.android_id || deviceId;
    } catch (error) {
      console.warn('Failed to fetch device info:', error);
    }

    try {
      ip = await tauriBridge.getLocalIp();
    } catch (error) {
      console.warn('Failed to fetch local IP:', error);
    }
    return { deviceId, deviceName, ip };
  }

  private readonly handleBackButton = (): void => {
    if (window.location.hash !== '') return;
    const now = Date.now();
    if (now - this.lastBackPress < EXIT_CONFIRMATION_WINDOW_MS) {
      void import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) => getCurrentWindow().close())
        .catch(console.warn);
      return;
    }

    this.lastBackPress = now;
    useToastStore.getState().show('info', 'Press back again to exit');
    window.history.pushState(null, '', '#root');
  };

  private readonly handleVisibilityChange = (): void => {
    if (document.visibilityState === 'visible') {
      void connectionController.reconnectOnAppOpen().catch(console.warn);
    }
  };
}

export const applicationController = new ApplicationController();
