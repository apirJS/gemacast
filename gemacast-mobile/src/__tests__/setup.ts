import { beforeAll, afterEach, mock } from 'bun:test';

const _store: Map<string, string> = new Map();

beforeAll(() => {
  if (typeof globalThis.localStorage === 'undefined') {
    (globalThis as any).localStorage = {
      getItem: (key: string) => _store.get(key) ?? null,
      setItem: (key: string, value: string) => { _store.set(key, value); },
      removeItem: (key: string) => { _store.delete(key); },
      clear: () => { _store.clear(); },
      get length() { return _store.size; },
      key: (_index: number) => null,
    } as Storage;
  }

  if (typeof globalThis.navigator === 'undefined') {
    (globalThis as any).navigator = { onLine: true } as Navigator;
  }

  if (typeof globalThis.window === 'undefined') {
    (globalThis as any).window = {
      addEventListener: () => {},
      removeEventListener: () => {},
      setInterval: globalThis.setInterval,
      clearInterval: globalThis.clearInterval,
    } as any;
  }
});

afterEach(() => {
  _store.clear();
});

export let invokeCalls: Array<{ cmd: string; args?: unknown }> = [];
let currentHandlers: Record<string, unknown | (() => unknown)> = {};

export function setupInvokeMock(
  handlers: Record<string, unknown | (() => unknown)> = {},
) {
  invokeCalls = [];
  currentHandlers = handlers;
}

mock.module('@tauri-apps/api/core', () => ({
  invoke: async (cmd: string, args?: unknown) => {
    invokeCalls.push({ cmd, args });
    const h = currentHandlers[cmd];
    if (typeof h === 'function') return (h as () => unknown)();
    if (h !== undefined) return h;
    return undefined;
  },
}));

mock.module('@tauri-apps/api/event', () => ({
  listen: async () => () => {},
}));

mock.module('tauri-plugin-device-info-api', () => ({
  getDeviceInfo: async () => ({
    device_name: 'Test Phone',
    manufacturer: 'Test',
    model: 'Phone',
    uuid: 'test-uuid',
    android_id: 'test-android-id',
  }),
}));

export function makeDeviceInfo(overrides: Partial<{ deviceId: string; deviceName: string; ip: string }> = {}) {
  return {
    deviceId: 'test-device-id',
    deviceName: 'Test Phone',
    ip: '192.168.1.100',
    ...overrides,
  };
}

export function makeDiscoveredSender(overrides: Partial<{ deviceId: string; deviceName: string; addr: string; isOffline: boolean }> = {}) {
  return {
    deviceId: 'pc-sender-1',
    deviceName: 'Desktop PC',
    addr: '192.168.1.10:9000',
    isOffline: false,
    ...overrides,
  };
}
