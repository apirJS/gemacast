import { tauriBridge } from '../core/tauri-bridge';
import { ConnectionMode, Status } from '../core/types';
import { useAppStore } from '../stores/app-store';
import { useToastStore } from '../stores/toast-store';
import { connectionController } from './connection-controller';
import { discoveryController } from './discovery-controller';

const NETWORK_POLL_MS = 3000;

function hasActiveSession(status: Status): boolean {
  return [Status.Connected, Status.Playing, Status.Paused, Status.Reconnecting].includes(status);
}

type NetworkInformation = EventTarget;

export class NetworkController {
  private networkId = '';
  private selectedMode: ConnectionMode | null = null;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private networkInformation: NetworkInformation | null = null;
  private unsubscribeStore: (() => void) | null = null;

  start(): () => void {
    this.selectedMode = useAppStore.getState().settings.mode;
    void this.refresh();
    if (document.visibilityState === 'visible') this.startPolling();

    window.addEventListener('online', this.handleOnline);
    window.addEventListener('offline', this.handleOffline);
    document.addEventListener('visibilitychange', this.handleVisibilityChange);

    const navigatorWithConnection = navigator as Navigator & {
      connection?: NetworkInformation;
      mozConnection?: NetworkInformation;
      webkitConnection?: NetworkInformation;
    };
    this.networkInformation =
      navigatorWithConnection.connection ??
      navigatorWithConnection.mozConnection ??
      navigatorWithConnection.webkitConnection ??
      null;
    this.networkInformation?.addEventListener('change', this.refresh);
    this.unsubscribeStore = useAppStore.subscribe((state) => {
      if (state.settings.mode === this.selectedMode) return;
      this.selectedMode = state.settings.mode;
      void this.switchMode(state.settings.mode);
    });

    return () => this.stop();
  }

  stop(): void {
    this.stopPolling();
    window.removeEventListener('online', this.handleOnline);
    window.removeEventListener('offline', this.handleOffline);
    document.removeEventListener('visibilitychange', this.handleVisibilityChange);
    this.networkInformation?.removeEventListener('change', this.refresh);
    this.networkInformation = null;
    this.unsubscribeStore?.();
    this.unsubscribeStore = null;
  }

  readonly refresh = async (): Promise<void> => {
    try {
      const network = await tauriBridge.getNetworkState();
      const state = useAppStore.getState();

      if (
        state.settings.mode === ConnectionMode.Wifi &&
        (state.deviceInfo.ip !== network.localIp || this.networkId !== network.networkId)
      ) {
        this.networkId = network.networkId;
        await this.handleNetworkChange(network.localIp);
      }

      useAppStore.getState().setAvailableModes(network.modes);
      const selectedModeUnavailable =
        (state.settings.mode === ConnectionMode.Usb && !network.modes.usb) ||
        (state.settings.mode === ConnectionMode.Wifi && !network.modes.wifi);
      if (selectedModeUnavailable && hasActiveSession(state.status)) {
        void connectionController.disconnect(true);
        connectionController.killPlayback();
      }
    } catch {
      // A later poll retries transient platform failures.
    }
  };

  private async handleNetworkChange(localIp: string): Promise<void> {
    const state = useAppStore.getState();
    if (hasActiveSession(state.status)) {
      try {
        await connectionController.disconnect(true);
      } catch (error) {
        console.warn('[NetworkController] Disconnect after network change failed:', error);
      }
    }

    connectionController.killPlayback();
    useAppStore.getState().dismissError();
    useAppStore.getState().patch({
      deviceInfo: { ...state.deviceInfo, ip: localIp },
      discoveredStreamers: [],
      connectedStreamer: null,
      lastConnectedStreamer: state.lastConnectedStreamer ?? state.connectedStreamer,
      status: Status.Listening,
    });
    await discoveryController.stop();
    void discoveryController.start(state.settings.mode);
  }

  private async switchMode(mode: ConnectionMode): Promise<void> {
    const state = useAppStore.getState();
    if (hasActiveSession(state.status)) {
      try {
        await connectionController.disconnect(true);
      } catch (error) {
        console.warn('[NetworkController] Mode switch cleanup failed:', error);
      }
      connectionController.killPlayback();
    }

    useAppStore.getState().dismissError();
    useAppStore.getState().patch({ discoveredStreamers: [], status: Status.Listening });
    await discoveryController.stop();
    void discoveryController.start(mode);
  }

  private readonly handleOnline = (): void => {
    const state = useAppStore.getState();
    if (state.settings.mode === ConnectionMode.Wifi) {
      state.dismissError();
      state.setNetworkAvailable(true);
      useToastStore.getState().show('info', 'Network online');
    }
    void this.refresh();
  };

  private readonly handleOffline = (): void => {
    const state = useAppStore.getState();
    if (state.settings.mode !== ConnectionMode.Wifi) return;

    state.patch({
      isNetworkAvailable: false,
      connectionHealth: 'lost',
      discoveredStreamers: [],
    });
    useToastStore.getState().show('warning', 'Network offline');

    if (
      state.connectedStreamer ||
      state.status === Status.Playing ||
      state.status === Status.Paused
    ) {
      state.patch({ status: Status.Listening, connectedStreamer: null });
      state.resetMetrics();
      connectionController.killPlayback();
    }
  };

  private readonly handleVisibilityChange = (): void => {
    if (document.visibilityState === 'visible') {
      void this.refresh();
      this.startPolling();
    } else {
      this.stopPolling();
    }
  };

  private startPolling(): void {
    this.pollTimer ??= setInterval(() => void this.refresh(), NETWORK_POLL_MS);
  }

  private stopPolling(): void {
    if (this.pollTimer === null) return;
    clearInterval(this.pollTimer);
    this.pollTimer = null;
  }
}

export const networkController = new NetworkController();
