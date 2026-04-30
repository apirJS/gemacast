import {
  getDeviceInfo,
  DeviceInfoResponse,
} from 'tauri-plugin-device-info-api';
import { invoke } from '@tauri-apps/api/core';
import { DeviceInfo, ConnectionMode, Status } from './types';
import { StateHandler } from './core/StateHandler';
import { DiscoveryService } from './core/DiscoveryService';
import { ConnectionService } from './core/ConnectionService';
import { AudioService } from './core/AudioService';
import { LatencyTracker } from './core/LatencyTracker';

const LS_DEVICE_ID = 'gemacast_device_id';

export class App {
  public readonly stateHandler: StateHandler;
  public readonly discovery: DiscoveryService;
  public readonly connection: ConnectionService;
  public readonly audio: AudioService;
  public readonly latency: LatencyTracker;
  private lastMode: ConnectionMode;
  private currentNetworkIdentifier: string = '';

  private constructor(deviceInfo: DeviceInfo, initialNetworkId: string) {
    this.currentNetworkIdentifier = initialNetworkId;
    this.stateHandler = new StateHandler(deviceInfo);
    this.lastMode = this.stateHandler.getState().settings.mode;
    this.audio = new AudioService(this.stateHandler);
    this.connection = new ConnectionService(this.stateHandler, () =>
      this.audio.startAudioPlayback().then(() => {}),
    );

    this.discovery = new DiscoveryService(this.stateHandler, (sender) => {
      this.connection.connectToSender(sender);
    });

    this.latency = new LatencyTracker(this.stateHandler);

    this.setupModeChangeObserver();
    this.setupNetworkListeners();
  }

  private setupModeChangeObserver() {
    this.stateHandler.subscribe(async (state) => {
      const currentMode = state.settings.mode;
      if (currentMode !== this.lastMode) {
        this.lastMode = currentMode;

        if (
          state.status === Status.Connected ||
          state.status === Status.Playing ||
          state.status === Status.Reconnecting
        ) {
          try {
            await this.connection.disconnect(true);
            await this.connection.killPlayback();
          } catch (e) {
            console.warn('[App] Cleanup during mode switch failed:', e);
          }
        }

        this.stateHandler.setState({
          discoveredSenders: [],
          error: null,
          status: Status.Listening,
        });

        await this.discovery.stopListening();
        this.discovery.startListening(currentMode);
      }
    });
  }

  private setupNetworkListeners() {
    const checkNetwork = async () => {
      try {
        const localIp = await invoke<string>('get_local_ip');
        const networkId = await invoke<string>('get_network_identifier').catch(
          () => localIp,
        );
        const modes = await invoke<{
          wifi: boolean;
          usb: boolean;
          adb: boolean;
        }>('get_connection_status');

        const currentState = this.stateHandler.getState();

        if (
          currentState.settings.mode !== ConnectionMode.Adb &&
          (currentState.deviceInfo.ip !== localIp ||
            this.currentNetworkIdentifier !== networkId)
        ) {
          this.currentNetworkIdentifier = networkId;

          if (
            currentState.status === Status.Playing ||
            currentState.status === Status.Connected
          ) {
            try {
              await this.connection.disconnect(true);
            } catch (e) {
              console.warn(
                '[App] Graceful disconnect on network hop failed:',
                e,
              );
            }
          }

          await this.connection.killPlayback();

          StateHandler.saveLastSender(null);

          this.stateHandler.setState({
            deviceInfo: { ...currentState.deviceInfo, ip: localIp },
            discoveredSenders: [],
            connectedSender: null,
            lastConnectedSender: null,
            error: null,
            status: Status.Listening,
          });

          await this.discovery.stopListening();
          this.discovery.startListening(currentState.settings.mode);
        }

        const wifiAvailable = modes.wifi;
        const usbAvailable = modes.usb;

        this.stateHandler.setState({
          availableModes: {
            wifi: wifiAvailable,
            usb: usbAvailable,
            adb: modes.adb,
          },
        });

        let currentMode = currentState.settings.mode;

        if (
          currentMode === ConnectionMode.Usb &&
          !usbAvailable &&
          currentState.status === Status.Playing
        ) {
          this.connection.disconnect(true);
          this.connection.killPlayback();
        } else if (
          currentMode === ConnectionMode.Wifi &&
          !wifiAvailable &&
          currentState.status === Status.Playing
        ) {
          this.connection.disconnect(true);
          this.connection.killPlayback();
        }
      } catch (e) {}
    };

    checkNetwork();
    setInterval(checkNetwork, 1000);

    window.addEventListener('online', checkNetwork);
    window.addEventListener('offline', checkNetwork);

    const connection =
      (navigator as any).connection ||
      (navigator as any).mozConnection ||
      (navigator as any).webkitConnection;
    if (connection) {
      connection.addEventListener('change', checkNetwork);
    }
  }

  private static generateUuid(): string {
    if (typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID();
    }
    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
      const arr = new Uint8Array(1);
      const r = crypto.getRandomValues
        ? (crypto.getRandomValues(arr), arr[0] % 16)
        : (Math.random() * 16) | 0;
      const v = c === 'x' ? r : (r & 0x3) | 0x8;
      return v.toString(16);
    });
  }

  private static getFallbackUuid(): string {
    let deviceId = localStorage.getItem(LS_DEVICE_ID);
    if (!deviceId) {
      deviceId = App.generateUuid();
      localStorage.setItem(LS_DEVICE_ID, deviceId);
    }
    return deviceId;
  }

  public static async create(): Promise<App> {
    let bestName = 'Unknown Android Device';
    let finalUuid = App.getFallbackUuid();
    let localIp = '127.0.0.1';
    let initialNetworkId = '';

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
      localIp = await invoke<string>('get_local_ip');
      initialNetworkId = await invoke<string>('get_network_identifier').catch(
        () => localIp,
      );
    } catch (e) {
      console.warn('Failed to fetch local IP & Network ID:', e);
    }

    const app = new App(
      {
        deviceId: finalUuid,
        deviceName: bestName,
        ip: localIp,
      },
      initialNetworkId,
    );

    try {
      const modes = await invoke<{ wifi: boolean; usb: boolean; adb: boolean }>(
        'get_connection_status',
      );

      app.stateHandler.setState({
        availableModes: { ...modes },
      });
    } catch (e) {
      console.warn('Failed to fetch initial connection status:', e);
    }

    return app;
  }
}
