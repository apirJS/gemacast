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
  getPairingDecisionWarning,
  handleSenderTimeout,
  handleForceDisconnect,
  handleLinkLost,
  handleLinkRecovered,
  changeAudioSource,
  isTerminalConnectError,
} from './use-connection';
import { ErrorCode } from '../core/error';
import { useToastStore } from '../stores/toast-store';

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
    start_link_recovery: undefined,
    stop_link_recovery: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
  useAppStore.getState().setStatus(Status.Listening);
  useToastStore.setState({ toasts: [] });
});

describe('connectToSender', () => {
  it('classifies pairing decisions as terminal', () => {
    expect(isTerminalConnectError('sender rejected the request (pairing_rejected)')).toBe(true);
    expect(isTerminalConnectError(new Error('HTTP request failed'))).toBe(false);
  });

  it('maps pairing decisions to concise warnings', () => {
    expect(getPairingDecisionWarning('sender rejected the request (pairing_rejected)')).toBe(
      'Pairing request rejected on the PC',
    );
    expect(getPairingDecisionWarning(new Error('pairing was cancelled on the phone'))).toBe(
      'Pairing cancelled',
    );
    expect(getPairingDecisionWarning('HTTP request failed')).toBeNull();
  });

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

  it('does not retry a rejected LAN pairing', async () => {
    let attempts = 0;
    setupInvokeMock({
      connect_to_sender: () => {
        attempts += 1;
        throw new Error('sender rejected the request (pairing_rejected)');
      },
    });

    await connectToSender(makeDiscoveredSender());

    expect(attempts).toBe(1);
  });

  it('shows a warning instead of a playback error when the PC rejects pairing', async () => {
    setupInvokeMock({
      connect_to_sender: () => {
        throw new Error('sender rejected the request (pairing_rejected)');
      },
    });

    const result = await connectToSender(makeDiscoveredSender());

    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0]).toMatchObject({
      type: 'warning',
      message: 'Pairing request rejected on the PC',
    });
  });

  it('shows a warning instead of a playback error when pairing is cancelled on the phone', async () => {
    setupInvokeMock({
      connect_to_sender: () => {
        throw new Error('PC identity confirmation was cancelled on the phone');
      },
    });

    const result = await connectToSender(makeDiscoveredSender());

    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).toBeNull();
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0]).toMatchObject({
      type: 'warning',
      message: 'Pairing cancelled',
    });
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

  it('resets metrics to all-null', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Connected,
    });
    useAppStore.getState().updateMetrics({ bufferMs: 10, networkRttMs: 12, jitterMs: 5 });
    await disconnect();
    const { metrics } = useAppStore.getState();
    expect(metrics.bufferMs).toBeNull();
    expect(metrics.networkRttMs).toBeNull();
    expect(metrics.jitterMs).toBeNull();
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

describe('handleLinkLost', () => {
  it('starts link recovery for the sender that was lost', async () => {
    const sender = makeDiscoveredSender({ addr: '10.0.0.7:9000' });
    useAppStore.getState().patch({ connectedSender: sender, status: Status.Playing });

    await handleLinkLost();

    const call = invokeCalls.find((c) => c.cmd === 'start_link_recovery');
    expect(call).toBeTruthy();
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.7');
  });

  it('never forgets the sender — recovery has to have something to reconnect to', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      lastConnectedSender: sender,
      status: Status.Playing,
    });

    await handleLinkLost();

    expect(useAppStore.getState().connectedSender).toBeNull();
    expect(useAppStore.getState().lastConnectedSender?.deviceId).toBe(sender.deviceId);
    expect(useAppStore.getState().status).toBe(Status.Listening);
    expect(useAppStore.getState().isSuspended).toBe(true);
  });

  it('tears the session down before arming the prober', async () => {
    useAppStore
      .getState()
      .patch({ connectedSender: makeDiscoveredSender(), status: Status.Playing });

    await handleLinkLost();

    // kill_playback cancels recovery on the Rust side, so a prober armed
    // before it would be aborted by the very teardown it followed.
    const killIndex = invokeCalls.findIndex((c) => c.cmd === 'kill_playback');
    const startIndex = invokeCalls.findIndex((c) => c.cmd === 'start_link_recovery');
    expect(killIndex).toBeGreaterThanOrEqual(0);
    expect(startIndex).toBeGreaterThan(killIndex);
  });

  it('does nothing when there was no live session to lose', async () => {
    useAppStore.getState().patch({ connectedSender: null, status: Status.Listening });

    await handleLinkLost();

    expect(invokeCalls.find((c) => c.cmd === 'start_link_recovery')).toBeUndefined();
  });
});

describe('handleLinkRecovered', () => {
  it('reconnects to the sender the link was lost from', async () => {
    const sender = makeDiscoveredSender({ addr: '10.0.0.8:9000' });
    useAppStore.getState().patch({
      lastConnectedSender: sender,
      status: Status.Listening,
      isSuspended: true,
    });

    await handleLinkRecovered(true);

    expect(useAppStore.getState().status).toBe(Status.Connected);
    const call = invokeCalls.find((c) => c.cmd === 'connect_to_sender');
    expect((call?.args as Record<string, unknown>).ip).toBe('10.0.0.8');
  });

  it('reconnects the same way whether or not the PC still had us registered', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({ lastConnectedSender: sender, status: Status.Listening });
    await handleLinkRecovered(false);

    // `deviceRegistered` is observability only today: both answers take the
    // full handshake, so neither can skip it.
    expect(invokeCalls.filter((c) => c.cmd === 'connect_to_sender')).toHaveLength(1);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });

  it('does not reconnect on top of a session the user already restored', async () => {
    useAppStore.getState().patch({
      lastConnectedSender: makeDiscoveredSender(),
      connectedSender: makeDiscoveredSender(),
      status: Status.Connected,
    });

    await handleLinkRecovered(true);

    expect(invokeCalls.find((c) => c.cmd === 'connect_to_sender')).toBeUndefined();
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
