import { describe, it, expect, beforeEach } from 'bun:test';
import { makeDeviceInfo, makeDiscoveredSender } from '../__tests__/setup';
import { useAppStore } from './app-store';
import { Status } from '../core/types';
import { GemaCastError, ErrorCode } from '../core/error';

beforeEach(() => {
  useAppStore.getState().init(makeDeviceInfo());
});

describe('app-store — initialization', () => {
  it('initializes with Idle status', () => {
    expect(useAppStore.getState().status).toBe(Status.Idle);
  });

  it('stores device info', () => {
    expect(useAppStore.getState().deviceInfo.deviceName).toBe('Test Phone');
    expect(useAppStore.getState().deviceInfo.ip).toBe('192.168.1.100');
  });

  it('starts with null connected sender', () => {
    expect(useAppStore.getState().connectedSender).toBeNull();
  });
});

describe('app-store — status transitions', () => {
  it('transitions to Listening', () => {
    useAppStore.getState().setStatus(Status.Listening);
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('transitions through Connecting → Connected', () => {
    useAppStore.getState().setStatus(Status.Connecting);
    expect(useAppStore.getState().status).toBe(Status.Connecting);
    useAppStore.getState().setStatus(Status.Connected);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });
});

describe('app-store — discovered senders', () => {
  it('adds a new sender', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().updateDiscoveredSender(sender);
    expect(useAppStore.getState().discoveredSenders).toHaveLength(1);
    expect(useAppStore.getState().discoveredSenders[0].deviceId).toBe(sender.deviceId);
  });

  it('updates an existing sender', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().updateDiscoveredSender(sender);
    const updated = { ...sender, deviceName: 'New Name' };
    useAppStore.getState().updateDiscoveredSender(updated);
    expect(useAppStore.getState().discoveredSenders).toHaveLength(1);
    expect(useAppStore.getState().discoveredSenders[0].deviceName).toBe('New Name');
  });

  it('removes an offline sender', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().updateDiscoveredSender(sender);
    useAppStore.getState().updateDiscoveredSender({ ...sender, isOffline: true });
    expect(useAppStore.getState().discoveredSenders).toHaveLength(0);
  });

  it('clears connected sender when it goes offline', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().setConnectedSender(sender);
    useAppStore.getState().updateDiscoveredSender(sender);
    useAppStore.getState().updateDiscoveredSender({ ...sender, isOffline: true });
    expect(useAppStore.getState().connectedSender).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('returns auto-reconnect target when last connected sender reappears', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedSender: sender,
      isSuspended: false,
    });
    const result = useAppStore.getState().updateDiscoveredSender(sender);
    expect(result?.deviceId).toBe(sender.deviceId);
  });

  it('does not auto-reconnect when suspended', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      status: Status.Listening,
      lastConnectedSender: sender,
      isSuspended: true,
    });
    const result = useAppStore.getState().updateDiscoveredSender(sender);
    expect(result).toBeNull();
  });
});

describe('app-store — error handling', () => {
  it('displays a GemaCastError', () => {
    const error = GemaCastError.senderTimeout();
    useAppStore.getState().displayError(error);
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.NETWORK_SENDER_TIMEOUT);
  });

  it('displays a string error by wrapping it', () => {
    useAppStore.getState().displayError('something broke');
    expect(useAppStore.getState().error).toBeInstanceOf(GemaCastError);
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.UNKNOWN_ERROR);
  });

  it('dismisses error', () => {
    useAppStore.getState().displayError(GemaCastError.senderTimeout());
    useAppStore.getState().dismissError();
    expect(useAppStore.getState().error).toBeNull();
  });
});

describe('app-store — latency', () => {
  it('updates latency stats', () => {
    useAppStore.getState().updateLatency({ current: 50, avg: 45, max: 80, min: 20 });
    const { latency } = useAppStore.getState();
    expect(latency.current).toBe(50);
    expect(latency.avg).toBe(45);
  });

  it('resets latency', () => {
    useAppStore.getState().updateLatency({ current: 50, avg: 45, max: 80, min: 20 });
    useAppStore.getState().resetLatency();
    const { latency } = useAppStore.getState();
    expect(latency.current).toBeNull();
    expect(latency.avg).toBeNull();
  });
});

describe('app-store — settings', () => {
  it('updates settings and persists', () => {
    useAppStore.getState().updateSettings({ bitratePreset: '256' });
    expect(useAppStore.getState().settings.bitratePreset).toBe('256');
    const saved = JSON.parse(localStorage.getItem('gemacast_settings')!);
    expect(saved.bitratePreset).toBe('256');
  });
});
