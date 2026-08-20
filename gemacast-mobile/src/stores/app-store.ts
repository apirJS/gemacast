import { create } from 'zustand';
import type {
  AppState,
  AppSettings,
  AudioSource,
  ConnectionHealth,
  DeviceInfo,
  DiscoveredSender,
  Metrics,
  NetworkLinkPairInfo,
  NotificationPermission,
  ProcessInfo,
  SenderCapabilities,
} from '../core/types';
import { Status, ConnectionMode } from '../core/types';
import { GemaCastError } from '../core/error';
import { loadLastSender, loadSettings, rememberPcName, saveSettings } from '../core/persistence';
import { useToastStore } from './toast-store';

const EMPTY_METRICS: Metrics = { bufferMs: null, networkRttMs: null, jitterMs: null };

function createInitialState(deviceInfo: DeviceInfo): AppState {
  return {
    deviceInfo,
    status: Status.Idle,
    discoveredSenders: [],
    connectedSender: null,
    connectingSenderId: null,
    lastConnectedSender: loadLastSender(),
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
    senderCapabilities: null,
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

  setDiscoveredSenders: (senders: DiscoveredSender[]) => void;
  updateDiscoveredSender: (sender: DiscoveredSender) => DiscoveredSender | null;

  setConnectedSender: (sender: DiscoveredSender | null) => void;
  setConnectingSenderId: (id: string | null) => void;
  setLastConnectedSender: (sender: DiscoveredSender | null) => void;
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
  setSenderCapabilities: (caps: SenderCapabilities | null) => void;
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

  setDiscoveredSenders: (senders) => set({ discoveredSenders: senders }),

  updateDiscoveredSender: (sender) => {
    const state = get();
    const list = [...state.discoveredSenders];
    const index = list.findIndex((s) => s.deviceId === sender.deviceId);

    let connectedSender = state.connectedSender;

    if (sender.isOffline) {
      if (index >= 0) list.splice(index, 1);

      if (state.connectedSender?.deviceId === sender.deviceId) {
        set({
          discoveredSenders: list,
          connectedSender: null,
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
      rememberPcName(sender.deviceId, sender.deviceName);

      if (index >= 0) {
        list[index] = sender;
      } else {
        list.push(sender);
      }
      if (connectedSender?.deviceId === sender.deviceId) {
        connectedSender = sender;
      }
    }

    set({ discoveredSenders: list, connectedSender });

    if (
      !sender.isOffline &&
      state.status === Status.Listening &&
      state.lastConnectedSender?.deviceId === sender.deviceId &&
      !state.isSuspended
    ) {
      return sender;
    }

    return null;
  },

  setConnectedSender: (sender) => set({ connectedSender: sender }),
  setConnectingSenderId: (id) => set({ connectingSenderId: id }),
  setLastConnectedSender: (sender) => set({ lastConnectedSender: sender }),
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
  setSenderCapabilities: (caps) => set({ senderCapabilities: caps }),
  setProcessList: (list) => set({ processList: list }),
  setNetworkLinkPair: (pair) => set({ networkLinkPair: pair }),
  setExclusiveSupported: (supported) => set({ exclusiveSupported: supported }),
  setNotificationPermission: (permission) => set({ notificationPermission: permission }),

  patch: (partial) => set(partial),
}));
