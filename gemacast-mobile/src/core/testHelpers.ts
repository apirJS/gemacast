import { mock } from 'bun:test';
import type { AppState, DeviceInfo, DiscoveredSender } from '../types';
import { Status } from '../types';

const _store: Map<string, string> = new Map();

export const localStorageMock = {
  getItem: (key: string) => _store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    _store.set(key, value);
  },
  removeItem: (key: string) => {
    _store.delete(key);
  },
  clear: () => {
    _store.clear();
  },
};

export function setupBrowserGlobals(onLine = true) {
  _store.clear();

  // @ts-expect-error – injecting global for tests
  globalThis.localStorage = localStorageMock;
  // @ts-expect-error
  globalThis.navigator = { onLine };
  globalThis.window = {
    // @ts-expect-error
    addEventListener: (_event: string, _cb: () => void) => {},
    // @ts-expect-error
    removeEventListener: (_event: string, _cb: () => void) => {},
  };
}

export let invokeCalls: Array<{ cmd: string; args?: unknown }> = [];

export function setupInvokeMock(
  handlers: Record<string, unknown | (() => unknown)> = {},
) {
  invokeCalls = [];

  mock.module('@tauri-apps/api/core', () => ({
    invoke: async (cmd: string, args?: unknown) => {
      invokeCalls.push({ cmd, args });
      const h = handlers[cmd];
      if (typeof h === 'function') return (h as () => unknown)();
      if (h !== undefined) return h;
      return undefined;
    },
  }));
}

export function makeDeviceInfo(
  overrides: Partial<DeviceInfo> = {},
): DeviceInfo {
  return {
    deviceId: 'test-device-id',
    deviceName: 'Test Phone',
    ip: '192.168.1.100',
    ...overrides,
  };
}

export function makeDiscoveredSender(
  overrides: Partial<DiscoveredSender> = {},
): DiscoveredSender {
  return {
    deviceId: 'pc-sender-1',
    deviceName: 'Desktop PC',
    addr: '192.168.1.10:9000',
    isOffline: false,
    ...overrides,
  };
}

export function makeAppState(overrides: Partial<AppState> = {}): AppState {
  return {
    deviceInfo: makeDeviceInfo(),
    status: Status.Idle,
    discoveredSenders: [],
    connectedSender: null,
    lastConnectedSender: null,
    error: null,
    connectionHealth: 'ok',
    isNetworkAvailable: true,
    isLoading: false,
    isSuspended: false,
    reconnectAttempts: 0,
    latency: { current: null, avg: null, max: null, min: null },
    settings: require('./StateHandler').DEFAULT_SETTINGS,
    availableModes: { wifi: true, usb: false, adb: false },
    audioSources: [],
    senderCapabilities: null,
    ...overrides,
  };
}
