import {
  getDeviceInfo,
  DeviceInfoResponse,
} from 'tauri-plugin-device-info-api';
import { AppState, DeviceInfo, StateSubscriber, Status, DiscoveredSender } from './types';
import { invoke } from '@tauri-apps/api/core';
import { err, ok, Result } from './types';
import { GemaCastError } from './error';

export class App {
  private state: AppState;
  private subscribers: StateSubscriber[] = [];

  private constructor(deviceInfo: DeviceInfo) {
    const savedVolume = parseFloat(
      localStorage.getItem('gemacast_volume') ?? '1.0',
    );

    this.state = {
      deviceInfo,
      status: Status.Idle,
      discoveredSenders: [],
      connectedSender: null,
      error: null,
      volume: isNaN(savedVolume) ? 1.0 : Math.max(0, Math.min(1, savedVolume)),
      isMuted: false,
    };
  }

  public getState(): AppState {
    return this.state;
  }

  public subscribe(callback: StateSubscriber): () => void {
    this.subscribers.push(callback);
    callback(this.state);

    return () => {
      this.subscribers = this.subscribers.filter((sub) => sub !== callback);
    };
  }

  private setState(partial: Partial<AppState>) {
    this.state = { ...this.state, ...partial };
    this.subscribers.forEach((callback) => callback(this.state));
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
    let deviceId = localStorage.getItem('device_id');
    if (!deviceId) {
      deviceId = App.generateUuid();
      localStorage.setItem('device_id', deviceId);
    }
    return deviceId;
  }

  public static async create(): Promise<App> {
    let bestName = 'Unknown Android Device';
    let finalUuid = App.getFallbackUuid();
    let localIp = '127.0.0.1';

    try {
      const rawInfo: DeviceInfoResponse = await getDeviceInfo();
      if (rawInfo.device_name) {
        bestName = rawInfo.device_name;
      } else if (rawInfo.manufacturer && rawInfo.model) {
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

  public async startListening(): Promise<Result<true, GemaCastError>> {
    try {
      await invoke('start_listening_for_senders');
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartDiscovery(e);
      this.setState({ error });
      return err(error);
    }
  }

  public async stopListening(): Promise<Result<true, GemaCastError>> {
    try {
      await invoke('stop_listening_for_senders');
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStopDiscovery(e);
      this.setState({ error });
      return err(error);
    }
  }

  public updateDiscoveredSender(sender: DiscoveredSender) {
    const list = [...this.state.discoveredSenders];
    const index = list.findIndex(s => s.deviceId === sender.deviceId);
    
    if (sender.isOffline) {
      if (index >= 0) list.splice(index, 1);
    } else {
      if (index >= 0) {
        list[index] = sender;
      } else {
        list.push(sender);
      }
    }
    this.setState({ discoveredSenders: list });
    
    if (sender.isOffline && this.state.connectedSender?.deviceId === sender.deviceId) {
      this.handleForceDisconnect();
    }
  }

  public async connectToSender(sender: DiscoveredSender): Promise<Result<true, GemaCastError>> {
    try {
      // Extra check: only ip
      const ip = sender.addr.split(':')[0];
      await invoke('connect_to_sender', {
        ip,
        deviceId: this.state.deviceInfo.deviceId,
        deviceName: this.state.deviceInfo.deviceName,
      });
      this.setState({ connectedSender: sender, status: Status.Connected });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartPlayback(e);
      this.setState({ error });
      return err(error);
    }
  }

  public async disconnect(): Promise<Result<true, GemaCastError>> {
    const sender = this.state.connectedSender;
    if (!sender) return ok(true);
    
    try {
      const ip = sender.addr.split(':')[0];
      await invoke('disconnect_from_sender', {
        ip,
        deviceId: this.state.deviceInfo.deviceId
      });
      this.setState({ connectedSender: null, status: Status.Idle });
      return ok(true);
    } catch (e) {
      this.setState({ connectedSender: null, status: Status.Idle });
      return err(GemaCastError.from(e));
    }
  }

  public handleForceDisconnect() {
    this.setState({ connectedSender: null, status: Status.Idle });
  }

  public async setVolume(level: number): Promise<void> {
    const clamped = Math.max(0, Math.min(1, level));
    this.setState({ volume: clamped, isMuted: clamped === 0 });
    localStorage.setItem('gemacast_volume', String(clamped));
    try {
      await invoke('set_volume', { level: clamped });
    } catch (e) {
      console.warn('set_volume IPC failed:', e);
    }
  }

  public async toggleMute(): Promise<void> {
    const newMuted = !this.state.isMuted;
    // Send 0.0 when muting, restore saved volume when unmuting.
    const level = newMuted ? 0 : this.state.volume;
    this.setState({ isMuted: newMuted });
    try {
      await invoke('set_volume', { level });
    } catch (e) {
      console.warn('set_volume IPC failed:', e);
    }
  }

  public getVolume(): number {
    return this.state.volume;
  }

  public displayError(error: string | GemaCastError) {
    this.setState({
      error: error instanceof GemaCastError ? error : GemaCastError.from(error),
    });
  }

  public async startAudioPlayback() {
    try {
      await invoke('start_audio_playback');
      this.setState({ status: Status.Playing });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStartPlayback(e);
      this.setState({ error });
      return err(error);
    }
  }

  public async stopAudioPlayback() {
    try {
      await invoke('stop_audio_playback');
      this.setState({ status: Status.Connected });
      return ok(true);
    } catch (e) {
      const error = GemaCastError.failedToStopPlayback(e);
      this.setState({ error });
      return err(error);
    }
  }
}

