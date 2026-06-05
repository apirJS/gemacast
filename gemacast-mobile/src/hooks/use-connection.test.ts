import { describe, it, expect, beforeEach } from 'bun:test';
import {
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredSender,
} from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { Status } from '../core/types';
import {
  connectToSender,
  disconnect,
  handleSenderTimeout,
  handleForceDisconnect,
  changeAudioSource,
} from './use-connection';
import { ErrorCode } from '../core/error';

beforeEach(() => {
  setupInvokeMock({
    connect_to_sender: undefined,
    disconnect_from_sender: undefined,
    kill_playback: undefined,
    notify_streaming_stopped: undefined,
    get_audio_sources: [[], { supportsProcessCapture: false }],
    get_process_list: [],
    probe_sender: undefined,
    establish_websocket: undefined,
    change_audio_source: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
  useAppStore.getState().setStatus(Status.Listening);
});

describe('connectToSender', () => {
  it('transitions through Connecting → Connected on success', async () => {
    const sender = makeDiscoveredSender();
    const result = await connectToSender(sender);
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Connected);
    expect(useAppStore.getState().connectedSender?.deviceId).toBe(sender.deviceId);
    expect(useAppStore.getState().isLoading).toBe(false);
  });

  it('invokes connect_to_sender with correct IP', async () => {
    await connectToSender(makeDiscoveredSender({ addr: '10.0.0.1:9000' }));
    const call = invokeCalls.find((c) => c.cmd === 'connect_to_sender');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.1');
  });

  it('saves lastConnectedSender on connect', async () => {
    const sender = makeDiscoveredSender();
    await connectToSender(sender);
    expect(useAppStore.getState().lastConnectedSender?.deviceId).toBe(sender.deviceId);
  });

  it('returns err and reverts to Listening on IPC failure', async () => {
    setupInvokeMock({
      connect_to_sender: () => {
        throw new Error('refused');
      },
    });
    const result = await connectToSender(makeDiscoveredSender());
    expect(result.ok).toBe(false);
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().error).not.toBeNull();
  });

  it('resets reconnectAttempts to 0 on connect', async () => {
    useAppStore.getState().patch({ reconnectAttempts: 3 });
    await connectToSender(makeDiscoveredSender());
    expect(useAppStore.getState().reconnectAttempts).toBe(0);
  });
});

describe('disconnect', () => {
  it('transitions to Listening and clears connectedSender', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      status: Status.Connected,
    });
    const result = await disconnect();
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().connectedSender).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('invokes disconnect_from_sender IPC', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender({ addr: '10.0.0.2:9000' }),
      status: Status.Connected,
    });
    await disconnect();
    expect(invokeCalls.some((c) => c.cmd === 'disconnect_from_sender')).toBe(true);
  });

  it('still succeeds when no sender is connected', async () => {
    const result = await disconnect();
    expect(result.ok).toBe(true);
  });

  it('resets latency to all-null', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Connected,
    });
    useAppStore.getState().updateLatency({ current: 10, avg: 12, max: 20, min: 5 });
    await disconnect();
    const { latency } = useAppStore.getState();
    expect(latency.current).toBeNull();
    expect(latency.avg).toBeNull();
  });
});

describe('handleSenderTimeout', () => {
  it('removes the timed-out sender from the list', () => {
    const sender = makeDiscoveredSender({ deviceId: 'gone' });
    useAppStore.getState().setDiscoveredSenders([sender]);
    handleSenderTimeout('gone');
    expect(useAppStore.getState().discoveredSenders).toHaveLength(0);
  });

  it('enters Listening when connected sender times out', () => {
    const sender = makeDiscoveredSender({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedSender: sender,
      lastConnectedSender: sender,
      discoveredSenders: [sender],
      status: Status.Connected,
    });
    handleSenderTimeout('pc-1');
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().connectedSender).toBeNull();
  });

  it('sets a senderTimeout error', () => {
    const sender = makeDiscoveredSender({ deviceId: 'pc-1' });
    useAppStore.getState().patch({
      connectedSender: sender,
      discoveredSenders: [sender],
    });
    handleSenderTimeout('pc-1');
    expect(useAppStore.getState().error?.code).toBe(ErrorCode.NETWORK_SENDER_TIMEOUT);
  });
});

describe('handleForceDisconnect', () => {
  it('clears connectedSender and moves to Listening', () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Playing,
    });
    handleForceDisconnect();
    expect(useAppStore.getState().connectedSender).toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });

  it('forgets lastConnectedSender when forgetSender=true', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      lastConnectedSender: sender,
      status: Status.Connected,
    });
    handleForceDisconnect(true);
    expect(useAppStore.getState().lastConnectedSender).toBeNull();
  });

  it('retains lastConnectedSender when forgetSender=false', () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      lastConnectedSender: sender,
      status: Status.Connected,
    });
    handleForceDisconnect(false);
    expect(useAppStore.getState().lastConnectedSender?.deviceId).toBe(sender.deviceId);
  });
});

describe('changeAudioSource', () => {
  it('returns err when no sender connected', async () => {
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(false);
  });

  it('invokes change_audio_source IPC on success', async () => {
    setupInvokeMock({ change_audio_source: undefined });
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender({ addr: '10.0.0.5:9000' }),
      status: Status.Connected,
    });
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(true);
    const call = invokeCalls.find((c) => c.cmd === 'change_audio_source');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.5');
  });

  it('returns err on IPC failure', async () => {
    setupInvokeMock({
      change_audio_source: () => {
        throw new Error('denied');
      },
    });
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Connected,
    });
    const result = await changeAudioSource({ type: 'desktop' });
    expect(result.ok).toBe(false);
  });
});
