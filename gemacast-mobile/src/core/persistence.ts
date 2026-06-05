import type { AppSettings, DiscoveredSender } from './types';
import { ConnectionMode } from './types';
import { JITTER_PRESETS } from './presets';

const LS_LAST_SENDER = 'gemacast_last_sender';
const LS_SETTINGS = 'gemacast_settings';
const LS_DEVICE_ID = 'gemacast_device_id';

const DEFAULT_AUTO_CONFIG = JITTER_PRESETS.find((p) => p.id === 'auto')!.config!;

export const DEFAULT_SETTINGS: AppSettings = {
  theme: 'dark',
  mode: ConnectionMode.Wifi,
  exclusiveMode: true,
  bufferPreset: 'auto',
  customJitterConfig: DEFAULT_AUTO_CONFIG,
  savedPresets: [],
  bitratePreset: '128',
  customBitrateKbps: 128,
  gainDb: 0,
};

export function loadLastSender(): DiscoveredSender | null {
  try {
    const raw = localStorage.getItem(LS_LAST_SENDER);
    return raw ? (JSON.parse(raw) as DiscoveredSender) : null;
  } catch {
    return null;
  }
}

export function saveLastSender(sender: DiscoveredSender | null) {
  if (sender) {
    localStorage.setItem(LS_LAST_SENDER, JSON.stringify(sender));
  } else {
    localStorage.removeItem(LS_LAST_SENDER);
  }
}

export function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(LS_SETTINGS);
    if (raw) {
      return { ...DEFAULT_SETTINGS, ...JSON.parse(raw) };
    }
  } catch {
    // Ignore JSON parse errors
  }
  return DEFAULT_SETTINGS;
}

export function saveSettings(settings: AppSettings) {
  localStorage.setItem(LS_SETTINGS, JSON.stringify(settings));
}

export function generateUuid(): string {
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

export function getOrCreateDeviceId(): string {
  let deviceId = localStorage.getItem(LS_DEVICE_ID);
  if (!deviceId) {
    deviceId = generateUuid();
    localStorage.setItem(LS_DEVICE_ID, deviceId);
  }
  return deviceId;
}
