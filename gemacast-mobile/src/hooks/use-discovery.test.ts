import { describe, it, expect, beforeEach } from 'bun:test';
import { setupInvokeMock, invokeCalls, makeDeviceInfo } from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { Status, ConnectionMode } from '../core/types';
import { startListening, stopListening } from './use-discovery';

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
