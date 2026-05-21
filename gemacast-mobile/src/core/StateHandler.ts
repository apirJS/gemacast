import {
  AppState,
  StateSubscriber,
  Status,
  DeviceInfo,
  ConnectionHealth,
  DiscoveredSender,
  ConnectionMode,
  AppSettings,
} from '../types';
import { GemaCastError } from '../error';
import { JITTER_PRESETS } from './presets';

// Auto preset is the single source of truth for the default fallback config.
const DEFAULT_AUTO_CONFIG = JITTER_PRESETS.find(p => p.id === 'auto')!.config!;

const LS_LAST_SENDER = 'gemacast_last_sender';
const LS_SETTINGS = 'gemacast_settings';

export const DEFAULT_SETTINGS: AppSettings = {
  theme: 'dark',
  mode: ConnectionMode.Wifi,
  exclusiveMode: false,
  bufferPreset: 'auto',
  customJitterConfig: DEFAULT_AUTO_CONFIG,
  savedPresets: [],
};

export class StateHandler {
  private state: AppState;
  private subscribers: StateSubscriber[] = [];
  private pendingNotify = false;

  constructor(deviceInfo: DeviceInfo) {
    const lastConnectedSender = StateHandler.loadLastSender();
    const settings = StateHandler.loadSettings();

    this.state = {
      deviceInfo,
      status: Status.Idle,
      discoveredSenders: [],
      connectedSender: null,
      lastConnectedSender,
      error: null,
      connectionHealth: 'ok',
      isNetworkAvailable: navigator.onLine,
      isLoading: false,
      isSuspended: false,
      reconnectAttempts: 0,
      latency: { current: null, avg: null, max: null, min: null },
      settings,
      availableModes: { wifi: true, usb: false, adb: false },
      audioSources: [],
      senderCapabilities: null,
      processList: [],
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

  public setState(partial: Partial<AppState>) {
    this.state = { ...this.state, ...partial };
    if (partial.settings) {
      StateHandler.saveSettings(partial.settings);
    }
    // Coalesce multiple rapid setState() calls into one subscriber
    // notification per vsync frame. Prevents Android's BLASTBufferQueue
    // overflow when latency, network, and audio-active events fire in
    // quick succession — each triggering full DOM rebuilds.
    if (!this.pendingNotify) {
      this.pendingNotify = true;
      requestAnimationFrame(() => {
        this.pendingNotify = false;
        this.subscribers.forEach((cb) => cb(this.state));
      });
    }
  }

  public displayError(error: string | GemaCastError) {
    this.setState({
      error: error instanceof GemaCastError ? error : GemaCastError.from(error),
    });
  }

  public dismissError() {
    this.setState({ error: null });
  }

  public setConnectionHealth(health: ConnectionHealth) {
    this.setState({ connectionHealth: health });
  }

  public updateLatencyInfo(
    current: number | null,
    avg: number | null,
    max: number | null,
    min: number | null,
  ) {
    this.setState({
      latency: { current, avg, max, min },
    });
  }

  public static loadLastSender(): DiscoveredSender | null {
    try {
      const raw = localStorage.getItem(LS_LAST_SENDER);
      return raw ? (JSON.parse(raw) as DiscoveredSender) : null;
    } catch {
      return null;
    }
  }

  public static saveLastSender(sender: DiscoveredSender | null) {
    if (sender) {
      localStorage.setItem(LS_LAST_SENDER, JSON.stringify(sender));
    } else {
      localStorage.removeItem(LS_LAST_SENDER);
    }
  }

  public static loadSettings(): AppSettings {
    try {
      const raw = localStorage.getItem(LS_SETTINGS);
      if (raw) {
        return { ...DEFAULT_SETTINGS, ...JSON.parse(raw) };
      }
    } catch {}
    return DEFAULT_SETTINGS;
  }

  public static saveSettings(settings: AppSettings) {
    localStorage.setItem(LS_SETTINGS, JSON.stringify(settings));
  }
}
