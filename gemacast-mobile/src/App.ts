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
        console.log(
          `[App] Mode changed from ${this.lastMode} to ${currentMode} — refreshing state`,
        );
        this.lastMode = currentMode;

        // 1. Force hard disconnect if active
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

        // 2. Clear stale list and restart discovery
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
        const networkId = await invoke<string>('get_network_identifier').catch(() => localIp);
        const modes = await invoke<{ wifi: boolean; usb: boolean }>(
          'get_connection_status',
        );

        const currentState = this.stateHandler.getState();

        // 1. Update IP address & Handle Network Jumps
        if (currentState.deviceInfo.ip !== localIp || this.currentNetworkIdentifier !== networkId) {
          console.log(
            `[App] Network jumped (IP: ${localIp}, ID: ${networkId}) — triggering network purge`,
          );
          
          this.currentNetworkIdentifier = networkId;

          // If we were streaming, that connection is definitely dead now
          // Attempt a graceful disconnect so the PC stops blasting UDP instantly
          if (
            currentState.status === Status.Playing ||
            currentState.status === Status.Connected
          ) {
            try {
              // passing `true` implies `forgetSender`, stopping Discovery from re-latching
              await this.connection.disconnect(true);
            } catch (e) {
              console.warn('[App] Graceful disconnect on network hop failed:', e);
            }
          }
          
          // ALWAYS kill playback during network jumps to ensure backend socket doesn't ghost
          await this.connection.killPlayback();

          // Safety wipe from local storage so app reload doesn't trigger ghost connections
          StateHandler.saveLastSender(null);

          this.stateHandler.setState({
            deviceInfo: { ...currentState.deviceInfo, ip: localIp },
            discoveredSenders: [], // 🧹 IMMEDIATELY clear list on network jump
            connectedSender: null,
            lastConnectedSender: null, // 🛑 User requested: Do NOT auto-reconnect across BSSID/Interface changes
            error: null,
            status: Status.Listening, // Force back to scanning UI
          });

          // Restart discovery to bind to new interface address if necessary
          await this.discovery.stopListening();
          this.discovery.startListening(currentState.settings.mode);
        }

        // 2. Update Available Modes & Policy
        const wifiAvailable = modes.wifi;
        const usbAvailable = modes.usb;

        this.stateHandler.setState({
          availableModes: {
            wifi: wifiAvailable,
            usb: usbAvailable,
            adb: currentState.availableModes.adb,
          },
        });

        // 3. Fallback Policy: If current mode becomes unavailable, find better alternative
        let currentMode = currentState.settings.mode;
        let nextMode = currentMode;

        if (currentMode === ConnectionMode.Usb && !usbAvailable) {
          // USB lost -> Fallback to Wifi if possible, otherwise stay but it will be disabled in UI
          nextMode = wifiAvailable ? ConnectionMode.Wifi : ConnectionMode.Usb;
        } else if (currentMode === ConnectionMode.Wifi && !wifiAvailable) {
          // Wifi lost -> Fallback to USB if possible
          nextMode = usbAvailable ? ConnectionMode.Usb : ConnectionMode.Wifi;
        }

        if (nextMode !== currentMode) {
          console.log(
            `[App] Current mode ${currentMode} unavailable, falling back to ${nextMode}`,
          );
          this.stateHandler.setState({
            settings: { ...currentState.settings, mode: nextMode },
          });
          // Note: The mode observer will handle disconnection and discovery refresh
        }

        // 4. Emergency: If we are STILL in a dead mode while streaming (e.g. both lost)
        const isCurrentlyUsb = nextMode === ConnectionMode.Usb;
        if (
          isCurrentlyUsb &&
          !usbAvailable &&
          currentState.status === Status.Playing
        ) {
          this.connection.disconnect(true);
          this.connection.killPlayback();
        } else if (
          nextMode === ConnectionMode.Wifi &&
          !wifiAvailable &&
          currentState.status === Status.Playing
        ) {
          this.connection.disconnect(true);
          this.connection.killPlayback();
        }

      } catch (e) {
        // Silently ignore errors during polling
      }
    };

    // Initially check network
    checkNetwork();

    // Poll the network state every 1 second to ensure instantaneous
    // detection of USB Tethering or Wi-Fi drops (native JS events sometimes miss these)
    setInterval(checkNetwork, 1000);

    // Re-check on regular online/offline events
    window.addEventListener('online', checkNetwork);
    window.addEventListener('offline', checkNetwork);

    // Re-check on detailed connection changes (e.g. Wi-Fi <-> Cellular)
    const connection = (navigator as any).connection || (navigator as any).mozConnection || (navigator as any).webkitConnection;
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
      initialNetworkId = await invoke<string>('get_network_identifier').catch(() => localIp);
    } catch (e) {
      console.warn('Failed to fetch local IP & Network ID:', e);
    }

    const app = new App({
      deviceId: finalUuid,
      deviceName: bestName,
      ip: localIp,
    }, initialNetworkId);

    // Determine best initial mode based on current availability
    try {
      const modes = await invoke<{ wifi: boolean; usb: boolean }>(
        'get_connection_status',
      );
      const state = app.stateHandler.getState();
      let mode = state.settings.mode;

      if (modes.usb) {
        mode = ConnectionMode.Usb;
      } else if (!modes.wifi && mode === ConnectionMode.Wifi) {
        // Fallback if saved mode is wifi but wifi is off
        mode = ConnectionMode.Usb; // try usb anyway let it be disabled later if both off
      }

      app.stateHandler.setState({
        availableModes: { ...modes, adb: state.availableModes.adb },
        settings: { ...state.settings, mode },
      });
    } catch (e) {
      console.warn('Failed to fetch initial connection status:', e);
    }

    return app;
  }
}
