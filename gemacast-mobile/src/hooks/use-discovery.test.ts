import { describe, it, expect, beforeEach } from 'bun:test';
import {
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredSender,
} from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { Status, ConnectionMode } from '../core/types';
import { startListening, stopListening, refreshSenders } from './use-discovery';

beforeEach(() => {
  setupInvokeMock({
    start_listening_for_senders: undefined,
    stop_listening_for_senders: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
});

describe('startListening', () => {
  it('transitions to Listening on success', async () => {
    const result = await startListening(ConnectionMode.Wifi);
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().isLoading).toBe(false);
  });

  it('invokes start_listening_for_senders with deviceId and mode', async () => {
    await startListening(ConnectionMode.Wifi);
    const call = invokeCalls.find((c) => c.cmd === 'start_listening_for_senders');
    expect(call).toBeTruthy();
    const args = call?.args as Record<string, unknown>;
    expect(args.deviceId).toBe('test-device-id');
    expect(args.mode).toBe('wifi');
  });

  it('returns err on IPC failure and stores error', async () => {
    setupInvokeMock({
      start_listening_for_senders: () => {
        throw new Error('bind failed');
      },
    });
    const result = await startListening(ConnectionMode.Wifi);
    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).not.toBeNull();
  });
});

describe('stopListening', () => {
  it('transitions to Idle', async () => {
    useAppStore.getState().setStatus(Status.Listening);
    const result = await stopListening();
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Idle);
  });

  it('invokes stop_listening_for_senders IPC', async () => {
    await stopListening();
    expect(invokeCalls.some((c) => c.cmd === 'stop_listening_for_senders')).toBe(true);
  });
});

describe('refreshSenders', () => {
  it('drops network-discovered senders but keeps manual and connected ones', async () => {
    const connected = makeDiscoveredSender({ deviceId: 'pc-connected', deviceName: 'Studio' });
    const discovered = makeDiscoveredSender({ deviceId: 'pc-other', deviceName: 'Laptop' });
    const manual = makeDiscoveredSender({
      deviceId: 'manual-192.168.1.5',
      deviceName: '192.168.1.5',
    });
    useAppStore.getState().setConnectedSender(connected);
    useAppStore.getState().setDiscoveredSenders([discovered, connected, manual]);

    const result = await refreshSenders();

    expect(result.ok).toBe(true);
    expect(useAppStore.getState().discoveredSenders.map((s) => s.deviceId)).toEqual([
      'pc-connected',
      'manual-192.168.1.5',
    ]);
  });

  it('re-arms discovery by stopping before starting the listeners', async () => {
    await refreshSenders();

    const stopIndex = invokeCalls.findIndex((c) => c.cmd === 'stop_listening_for_senders');
    const startIndex = invokeCalls.findIndex((c) => c.cmd === 'start_listening_for_senders');
    expect(stopIndex).toBeGreaterThan(-1);
    expect(startIndex).toBeGreaterThan(stopIndex);

    const startCall = invokeCalls.find((c) => c.cmd === 'start_listening_for_senders');
    const args = startCall?.args as Record<string, unknown>;
    expect(args.deviceId).toBe('test-device-id');
    expect(args.mode).toBe('wifi');
  });

  it('moves an idle session to Listening', async () => {
    useAppStore.getState().setStatus(Status.Idle);
    await refreshSenders();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('leaves an active stream untouched', async () => {
    useAppStore.getState().setStatus(Status.Playing);
    await refreshSenders();
    expect(useAppStore.getState().status).toBe(Status.Playing);
  });

  it('returns err and stores the error when re-arming fails', async () => {
    setupInvokeMock({
      stop_listening_for_senders: undefined,
      start_listening_for_senders: () => {
        throw new Error('bind failed');
      },
    });
    const result = await refreshSenders();
    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).not.toBeNull();
  });
});
