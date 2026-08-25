import { create } from 'zustand';
import type {
  AppState,
  AppSettings,
  AudioSource,
  ConnectionHealth,
  DeviceInfo,
  DiscoveredStreamer,
  Metrics,
  NetworkLinkPairInfo,
  NotificationPermission,
  ProcessInfo,
  StreamerCapabilities,
} from '../core/types';
import { Status, ConnectionMode } from '../core/types';
import { GemaCastError } from '../core/error';
import { loadLastStreamer, loadSettings, rememberPcName, saveSettings } from '../core/persistence';
import { useToastStore } from './toast-store';

const EMPTY_METRICS: Metrics = { bufferMs: null, networkRttMs: null, jitterMs: null };

function createInitialState(deviceInfo: DeviceInfo): AppState {
  return {
    deviceInfo,
    status: Status.Idle,
    discoveredStreamers: [],
    connectedStreamer: null,
    connectingStreamerId: null,
    lastConnectedStreamer: loadLastStreamer(),
    error: null,
    connectionHealth: 'ok',
    isNetworkAvailable: typeof navigator !== 'undefined' ? navigator.onLine : true,
    isLoading: false,
    isSuspended: false,
    reconnectAttempts: 0,
    metrics: EMPTY_METRICS,
    settings: loadSettings(),
    availableModes: { wifi: true, usb: false, adb: false },
    audioSources: [],
    currentAudioSource: { type: 'desktop' } as AudioSource,
    streamerCapabilities: null,
    processList: [],
    networkLinkPair: null,
    exclusiveSupported: true,
    // Optimistic, like `exclusiveSupported`: assume nothing is wrong until the
    // startup probe answers, so the settings notice cannot flash on every launch.
    notificationPermission: 'notRequired',
  };
}

type AppActions = {
  init: (deviceInfo: DeviceInfo) => void;

  setStatus: (status: Status) => void;
  setLoading: (isLoading: boolean) => void;
  setSuspended: (isSuspended: boolean) => void;
  setNetworkAvailable: (available: boolean) => void;

  setDiscoveredStreamers: (streamers: DiscoveredStreamer[]) => void;
  updateDiscoveredStreamer: (streamer: DiscoveredStreamer) => DiscoveredStreamer | null;

  setConnectedStreamer: (streamer: DiscoveredStreamer | null) => void;
  setConnectingStreamerId: (id: string | null) => void;
  setLastConnectedStreamer: (streamer: DiscoveredStreamer | null) => void;
  setConnectionHealth: (health: ConnectionHealth) => void;
  setReconnectAttempts: (attempts: number) => void;

  displayError: (error: string | GemaCastError) => void;
  dismissError: () => void;

  updateMetrics: (patch: Partial<Metrics>) => void;
  resetMetrics: () => void;

  updateSettings: (patch: Partial<AppSettings>) => void;
  setAvailableModes: (modes: { wifi: boolean; usb: boolean; adb: boolean }) => void;

  setDeviceInfo: (info: Partial<DeviceInfo>) => void;

  setAudioSources: (sources: AudioSource[]) => void;
  setCurrentAudioSource: (source: AudioSource) => void;
  setStreamerCapabilities: (caps: StreamerCapabilities | null) => void;
  setProcessList: (list: ProcessInfo[]) => void;
  setNetworkLinkPair: (pair: NetworkLinkPairInfo | null) => void;
  setExclusiveSupported: (supported: boolean) => void;
  setNotificationPermission: (permission: NotificationPermission) => void;

  patch: (partial: Partial<AppState>) => void;
};

export type AppStore = AppState & AppActions;

const DEFAULT_DEVICE: DeviceInfo = {
  deviceId: '',
  deviceName: 'Unknown',
  ip: '127.0.0.1',
};

export const useAppStore = create<AppStore>((set, get) => ({
  ...createInitialState(DEFAULT_DEVICE),

  init: (deviceInfo) => set(createInitialState(deviceInfo)),

  setStatus: (status) => set({ status }),
  setLoading: (isLoading) => set({ isLoading }),
  setSuspended: (isSuspended) => set({ isSuspended }),
  setNetworkAvailable: (available) => set({ isNetworkAvailable: available }),

  setDiscoveredStreamers: (streamers) => set({ discoveredStreamers: streamers }),

  updateDiscoveredStreamer: (streamer) => {
    const state = get();
    const list = [...state.discoveredStreamers];
    const index = list.findIndex((s) => s.deviceId === streamer.deviceId);

    let connectedStreamer = state.connectedStreamer;

    if (streamer.isOffline) {
      if (index >= 0) list.splice(index, 1);

      if (state.connectedStreamer?.deviceId === streamer.deviceId) {
        set({
          discoveredStreamers: list,
          connectedStreamer: null,
          status: Status.Listening,
          connectionHealth: 'ok',
          reconnectAttempts: 0,
          metrics: EMPTY_METRICS,
        });
        return null;
      }
    } else {
      // Cache the name while we have it: this packet is the only place a PC's
      // display name enters the app, and it outlives the discovery list.
      rememberPcName(streamer.deviceId, streamer.deviceName);

      if (index >= 0) {
        list[index] = streamer;
      } else {
        list.push(streamer);
      }
      if (connectedStreamer?.deviceId === streamer.deviceId) {
        connectedStreamer = streamer;
      }
    }

    set({ discoveredStreamers: list, connectedStreamer });

    if (
      !streamer.isOffline &&
      state.status === Status.Listening &&
      state.lastConnectedStreamer?.deviceId === streamer.deviceId &&
      !state.isSuspended
    ) {
      return streamer;
    }

    return null;
  },

  setConnectedStreamer: (streamer) => set({ connectedStreamer: streamer }),
  setConnectingStreamerId: (id) => set({ connectingStreamerId: id }),
  setLastConnectedStreamer: (streamer) => set({ lastConnectedStreamer: streamer }),
  setConnectionHealth: (health) => set({ connectionHealth: health }),
  setReconnectAttempts: (attempts) => set({ reconnectAttempts: attempts }),

  displayError: (error) => {
    const gemacastError = error instanceof GemaCastError ? error : GemaCastError.from(error);
    set({ error: gemacastError });
    useToastStore
      .getState()
      .show(
        'error',
        gemacastError.userMessage,
        `Code: ${gemacastError.code}\nMessage: ${gemacastError.message}\nCause: ${String(gemacastError.cause ?? 'Unknown')}`,
      );
  },

  dismissError: () => {
    set({ error: null });
    useToastStore.getState().clearError();
  },

  updateMetrics: (patch) => set((state) => ({ metrics: { ...state.metrics, ...patch } })),
  resetMetrics: () => set({ metrics: EMPTY_METRICS }),

  updateSettings: (patch) => {
    const current = get().settings;
    const updated = { ...current, ...patch };
    saveSettings(updated);
    set({ settings: updated });
  },

  setAvailableModes: (modes) => {
    const { settings, availableModes: prev } = get();
    // The network monitor calls this every 3s with a fresh object from IPC.
    // Only publish a new reference when a value actually changed, or every
    // `availableModes` subscriber (e.g. ModeSelector) re-renders each tick.
    if (prev.wifi !== modes.wifi || prev.usb !== modes.usb || prev.adb !== modes.adb) {
      set({ availableModes: modes });
    }

    const modeAvailable = (m: ConnectionMode) =>
      m === ConnectionMode.Wifi ? modes.wifi : m === ConnectionMode.Usb ? modes.usb : modes.adb;

    if (modeAvailable(settings.mode)) return;

    const prevAnyAvailable = prev.wifi || prev.usb || prev.adb;
    const nowAnyAvailable = modes.wifi || modes.usb || modes.adb;

    if (!nowAnyAvailable) return;

    const priority = [ConnectionMode.Wifi, ConnectionMode.Usb, ConnectionMode.Adb];
    const next = priority.find(modeAvailable);
    if (next && (prevAnyAvailable || nowAnyAvailable)) {
      const updated = { ...settings, mode: next };
      saveSettings(updated);
      set({ settings: updated });
    }
  },

  setDeviceInfo: (info) => {
    const current = get().deviceInfo;
    set({ deviceInfo: { ...current, ...info } });
  },

  setAudioSources: (sources) => set({ audioSources: sources }),
  setCurrentAudioSource: (source) => set({ currentAudioSource: source }),
  setStreamerCapabilities: (caps) => set({ streamerCapabilities: caps }),
  setProcessList: (list) => set({ processList: list }),
  setNetworkLinkPair: (pair) => set({ networkLinkPair: pair }),
  setExclusiveSupported: (supported) => set({ exclusiveSupported: supported }),
  setNotificationPermission: (permission) => set({ notificationPermission: permission }),

  patch: (partial) => set(partial),
}));
