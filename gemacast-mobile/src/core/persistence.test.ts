import { describe, it, expect, beforeEach } from 'bun:test';
import {
  saveLastSender,
  loadLastSender,
  saveSettings,
  loadSettings,
  getOrCreateDeviceId,
  generateUuid,
  DEFAULT_SETTINGS,
} from './persistence';

beforeEach(() => {
  localStorage.clear();
});

describe('lastSender persistence', () => {
  const sender = {
    deviceId: 'pc-1',
    deviceName: 'Desktop',
    addr: '192.168.1.10:9000',
    isOffline: false,
  };

  it('saveLastSender writes and loadLastSender reads back', () => {
    saveLastSender(sender);
    const loaded = loadLastSender();
    expect(loaded).toEqual(sender);
  });

  it('saveLastSender(null) removes the key', () => {
    saveLastSender(sender);
    saveLastSender(null);
    expect(loadLastSender()).toBeNull();
  });

  it('loadLastSender returns null when empty', () => {
    expect(loadLastSender()).toBeNull();
  });

  it('loadLastSender returns null on corrupt JSON', () => {
    localStorage.setItem('gemacast_last_sender', '{broken');
    expect(loadLastSender()).toBeNull();
  });
});

describe('settings persistence', () => {
  it('saveSettings writes and loadSettings reads back', () => {
    const custom = { ...DEFAULT_SETTINGS, theme: 'light' as const };
    saveSettings(custom);
    const loaded = loadSettings();
    expect(loaded.theme).toBe('light');
  });

  it('loadSettings merges partial save with defaults', () => {
    localStorage.setItem('gemacast_settings', JSON.stringify({ theme: 'light' }));
    const loaded = loadSettings();
    expect(loaded.theme).toBe('light');
    expect(loaded.mode).toBe(DEFAULT_SETTINGS.mode);
    expect(loaded.exclusiveMode).toBe(DEFAULT_SETTINGS.exclusiveMode);
  });

  it('loadSettings returns defaults when empty', () => {
    const loaded = loadSettings();
    expect(loaded).toEqual(DEFAULT_SETTINGS);
  });
});

describe('deviceId persistence', () => {
  describe('generateUuid', () => {
    it('generates a valid UUID using crypto.randomUUID if available', () => {
      const id = generateUuid();
      expect(typeof id).toBe('string');
      expect(id.length).toBe(36);
    });

    it('generates a valid UUID using fallback when randomUUID is undefined', () => {
      const originalRandomUUID = crypto.randomUUID;
      // @ts-expect-error Mocking for test
      crypto.randomUUID = undefined;
      const id = generateUuid();
      expect(typeof id).toBe('string');
      expect(id.length).toBe(36);
      crypto.randomUUID = originalRandomUUID;
    });
  });

  it('getOrCreateDeviceId creates and persists a UUID', () => {
    const id = getOrCreateDeviceId();
    expect(id).toBeTruthy();
    expect(typeof id).toBe('string');
    expect(localStorage.getItem('gemacast_device_id')).toBe(id);
  });

  it('getOrCreateDeviceId returns existing ID on subsequent calls', () => {
    const first = getOrCreateDeviceId();
    const second = getOrCreateDeviceId();
    expect(second).toBe(first);
  });

  it('generateUuid returns valid UUID format', () => {
    const uuid = generateUuid();
    expect(uuid).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });
});
