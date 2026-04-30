import * as z from 'zod';
import { GemaCastError } from './error';

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

export const deviceInfoSchema = z.object({
  deviceId: z.uuid(),
  deviceName: z.string(),
  ip: z.string(),
});

export type DeviceInfo = z.infer<typeof deviceInfoSchema>;

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
  Reconnecting = 'reconnecting',
}

export type ConnectionHealth = 'ok' | 'degraded' | 'lost';

export type LatencyStats = {
  /** Current ring-buffer fill time in ms (also the effective buffer time). */
  current: number | null;
  /** Rolling average over last 50 measurements. */
  avg: number | null;
  /** Maximum seen since connection was established. */
  max: number | null;
  /** Minimum seen since connection was established. */
  min: number | null;
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

export type SavedPreset = {
  name: string;
  config: JitterConfig;
};

export type PresetId = 'auto' | 'wired' | 'fast' | 'balanced' | 'stable' | 'resilient' | 'custom';

export type AppSettings = {
  theme: 'light' | 'dark';
  mode: ConnectionMode;
  exclusiveMode: boolean;
  bufferPreset: PresetId;
  customJitterConfig: JitterConfig;
  savedPresets: SavedPreset[];
};

export type AppState = {
  deviceInfo: DeviceInfo;
  status: Status;
  discoveredSenders: DiscoveredSender[];
  connectedSender: DiscoveredSender | null;
  /** Persisted across reconnects — used for auto-reconnect. Cleared on explicit user disconnect. */
  lastConnectedSender: DiscoveredSender | null;
  error: GemaCastError | null;
  connectionHealth: ConnectionHealth;
  isNetworkAvailable: boolean;
  isLoading: boolean;
  isSuspended: boolean;
  reconnectAttempts: number;
  latency: LatencyStats;
  settings: AppSettings;
  availableModes: { wifi: boolean; usb: boolean; adb: boolean };
};

export type StateSubscriber = (state: AppState) => void;
