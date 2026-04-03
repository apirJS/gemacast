import { getDeviceInfo, DeviceInfoResponse } from 'tauri-plugin-device-info-api';
import { invoke } from '@tauri-apps/api/core';
import { DeviceInfo } from './types';
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

  private constructor(deviceInfo: DeviceInfo) {
    this.stateHandler = new StateHandler(deviceInfo);
    
    this.audio = new AudioService(this.stateHandler);
    
    this.connection = new ConnectionService(this.stateHandler, () =>
      this.audio.startAudioPlayback().then(() => {})
    );

    this.discovery = new DiscoveryService(this.stateHandler, (sender) => {
      this.connection.connectToSender(sender);
    });

    this.latency = new LatencyTracker(this.stateHandler);
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
    } catch (e) {
      console.warn('Failed to fetch local IP:', e);
    }

    return new App({
      deviceId: finalUuid,
      deviceName: bestName,
      ip: localIp,
    });
  }
}
