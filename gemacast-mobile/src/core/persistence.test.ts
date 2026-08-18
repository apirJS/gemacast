import { describe, it, expect, beforeEach } from 'bun:test';
import {
  saveLastSender,
  loadLastSender,
  saveSettings,
  loadSettings,
  getOrCreateDeviceId,
  generateUuid,
  loadPcNames,
  rememberPcName,
  forgetPcName,
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

describe('paired PC name cache', () => {
  it('remembers and reads back a name', () => {
    rememberPcName('PC_abc', 'DESKTOP-KJCRNVV');
    expect(loadPcNames()['PC_abc']).toBe('DESKTOP-KJCRNVV');
  });

  it('returns an empty map when nothing is stored', () => {
    expect(loadPcNames()).toEqual({});
  });

  it('overwrites an existing name for the same id', () => {
    rememberPcName('PC_abc', 'OLD');
    rememberPcName('PC_abc', 'NEW');
    expect(loadPcNames()['PC_abc']).toBe('NEW');
  });

  it('ignores names that carry no information', () => {
    // A sender can arrive with an empty name, and the id is what we would fall
    // back to anyway — neither may evict a good cached name.
    rememberPcName('PC_abc', 'Good name');
    rememberPcName('PC_abc', '');
    rememberPcName('PC_abc', 'PC_abc');
    expect(loadPcNames()['PC_abc']).toBe('Good name');
  });

  it('forgets one name without touching the others', () => {
    rememberPcName('PC_a', 'One');
    rememberPcName('PC_b', 'Two');
    forgetPcName('PC_a');
    expect(loadPcNames()).toEqual({ PC_b: 'Two' });
  });

  it('forgetting an unknown id is a no-op', () => {
    rememberPcName('PC_a', 'One');
    forgetPcName('PC_missing');
    expect(loadPcNames()).toEqual({ PC_a: 'One' });
  });

  it('evicts the oldest entries past the cap', () => {
    for (let i = 0; i < 70; i++) rememberPcName(`PC_${i}`, `Name ${i}`);
    const names = loadPcNames();
    expect(Object.keys(names)).toHaveLength(64);
    expect(names['PC_0']).toBeUndefined();
    expect(names['PC_5']).toBeUndefined();
    expect(names['PC_6']).toBe('Name 6');
    expect(names['PC_69']).toBe('Name 69');
  });

  it('survives corrupt stored JSON', () => {
    localStorage.setItem('gemacast_pc_names', '{not json');
    expect(loadPcNames()).toEqual({});
    rememberPcName('PC_abc', 'Recovered');
    expect(loadPcNames()['PC_abc']).toBe('Recovered');
  });

  it('drops non-string values from a hand-edited store', () => {
    localStorage.setItem('gemacast_pc_names', JSON.stringify({ PC_a: 'One', PC_b: 42 }));
    expect(loadPcNames()).toEqual({ PC_a: 'One' });
  });
});
