import type { GemaCastError } from './error';

export type Ok<T> = {
  readonly ok: true;
  readonly value: T;
};

export type Err<E> = {
  readonly ok: false;
  readonly error: E;
};

export type Result<T, E = Error> = Ok<T> | Err<E>;

export function ok<T>(value: T): Ok<T> {
  return { ok: true, value };
}
export function err<E>(error: E): Err<E> {
  return { ok: false, error };
}

export type DeviceInfo = {
  deviceId: string;
  deviceName: string;
  ip: string;
};

export type DiscoveredSender = {
  deviceId: string;
  deviceName: string;
  addr: string;
  isOffline: boolean;
};

export enum Status {
  Idle = 'idle',
  Listening = 'listening',
  Connecting = 'connecting',
  Connected = 'connected',
  Playing = 'playing',
  Paused = 'paused',
  Reconnecting = 'reconnecting',
}

export type ConnectionHealth = 'ok' | 'degraded' | 'lost';

/**
 * The three live connection metrics shown in the UI. Each is milliseconds or
 * `null` when not yet measured. They are three genuinely distinct signals, not
 * views of one:
 * - `bufferMs`   — jitter-buffer dwell time (a frame's time from arrival to playback).
 * - `networkRttMs` — control-channel probe round-trip; `null` on ADB/loopback (no probe loop).
 * - `jitterMs`   — rolling network inter-arrival jitter estimate.
 */
export type Metrics = {
  bufferMs: number | null;
  networkRttMs: number | null;
  jitterMs: number | null;
};

export enum ConnectionMode {
  Wifi = 'wifi',
  Usb = 'usb',
  Adb = 'adb',
}

export type JitterConfig = {
  minDepthMs: number;
  comfortCapMs: number;
  peakDecayHalflifeMs: number;
  resumeThresholdPct: number;
  staticTargetMs?: number | null;
};

/** Detected network link type for one side of the connection. */
export type NetworkLink =
  | 'adb'
  | 'usbTether'
  | 'wifi5Ghz'
  | 'wifi2_4Ghz'
  | 'ethernet'
  | 'wifiUnknown'
  | 'unknown';

/** Network link pair info from the backend (both sides + effective). */
export type NetworkLinkPairInfo = {
  phone: NetworkLink;
  pc: NetworkLink;
  effective: NetworkLink;
  effectiveLabel: string;
};

export type SavedPreset = {
  name: string;
  config: JitterConfig;
};

export type PresetId =
  | 'nobuffer'
  | 'auto'
  | 'wired'
  | 'fast'
  | 'balanced'
  | 'stable'
  | 'resilient'
  | 'custom';

export type BitratePreset =
  | '10'
  | '24'
  | '32'
  | '64'
  | '96'
  | '128'
  | '256'
  | '450'
  | '512'
  | 'raw'
  | 'custom';

export type AppSettings = {
  theme: 'light' | 'dark';
  mode: ConnectionMode;
  exclusiveMode: boolean;
  keepScreenOn: boolean;
  bufferPreset: PresetId | string;
  customJitterConfig: JitterConfig;
  savedPresets: SavedPreset[];
  bitratePreset: BitratePreset;
  customBitrateKbps: number;
  gainDb: number;
};

export type AudioSource =
  | { type: 'desktop' }
  | { type: 'process'; pid: number; name: string; hasAudioSession?: boolean };

export type SenderCapabilities = {
  supportsProcessCapture: boolean;
};

export type ProcessInfo = {
  pid: number;
  name: string;
  hasAudioSession: boolean;
};

export type AppState = {
  deviceInfo: DeviceInfo;
  status: Status;
  discoveredSenders: DiscoveredSender[];
  connectedSender: DiscoveredSender | null;
  connectingSenderId: string | null;
  lastConnectedSender: DiscoveredSender | null;
  error: GemaCastError | null;
  connectionHealth: ConnectionHealth;
  isNetworkAvailable: boolean;
  isLoading: boolean;
  isSuspended: boolean;
  reconnectAttempts: number;
  metrics: Metrics;
  settings: AppSettings;
  availableModes: { wifi: boolean; usb: boolean; adb: boolean };
  audioSources: AudioSource[];
  currentAudioSource: AudioSource;
  senderCapabilities: SenderCapabilities | null;
  processList: ProcessInfo[];
  /** Detected network link pair from the active connection. */
  networkLinkPair: NetworkLinkPairInfo | null;
  /** Whether the device supports Oboe exclusive audio mode (probed at startup). */
  exclusiveSupported: boolean;
};
