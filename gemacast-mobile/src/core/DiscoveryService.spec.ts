import { describe, it, expect, beforeEach, mock } from 'bun:test';
import {
  setupBrowserGlobals,
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredSender,
} from './testHelpers';
import { StateHandler } from './StateHandler';
import { DiscoveryService } from './DiscoveryService';
import { Status, ConnectionMode } from '../types';
import type { DiscoveredSender } from '../types';
import { GemaCastError } from '../error';

function setup(handlers: Record<string, unknown> = {}) {
  setupInvokeMock(handlers);
  const sh = new StateHandler(makeDeviceInfo());
  const reconnectCb = mock(() => {});
  const discovery = new DiscoveryService(sh, reconnectCb);
  return { sh, discovery, reconnectCb };
}

beforeEach(() => {
  setupBrowserGlobals();
});

describe('DiscoveryService — startListening', () => {
  it('transitions to Listening on success', async () => {
    const { sh, discovery } = setup({ start_listening_for_senders: undefined });
    const result = await discovery.startListening(ConnectionMode.Wifi);
    expect(result.ok).toBe(true);
    expect(sh.getState().status).toBe(Status.Listening);
  });

  it('invokes start_listening_for_senders', async () => {
    const { discovery } = setup({ start_listening_for_senders: undefined });
    await discovery.startListening(ConnectionMode.Wifi);
    expect(invokeCalls[0]?.cmd).toBe('start_listening_for_senders');
  });

  it('returns err and sets error state on IPC failure', async () => {
    setupInvokeMock({
      start_listening_for_senders: () => {
        throw new Error('fail');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    const discovery = new DiscoveryService(
      sh,
      mock(() => {}),
    );
    const result = await discovery.startListening(ConnectionMode.Wifi);
    expect(result.ok).toBe(false);
    expect(sh.getState().error).toBeInstanceOf(GemaCastError);
    expect(sh.getState().isLoading).toBe(false);
  });
});

describe('DiscoveryService — stopListening', () => {
  it('transitions to Idle on success', async () => {
    const { sh, discovery } = setup({ stop_listening_for_senders: undefined });
    sh.setState({ status: Status.Listening });
    const result = await discovery.stopListening();
    expect(result.ok).toBe(true);
    expect(sh.getState().status).toBe(Status.Idle);
  });

  it('returns err on IPC failure', async () => {
    setupInvokeMock({
      stop_listening_for_senders: () => {
        throw new Error('net');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    const discovery = new DiscoveryService(
      sh,
      mock(() => {}),
    );
    const result = await discovery.stopListening();
    expect(result.ok).toBe(false);
  });
});

describe('DiscoveryService — updateDiscoveredSender', () => {
  it('adds a new sender to the list', () => {
    const { sh, discovery } = setup();
    discovery.updateDiscoveredSender(makeDiscoveredSender());
    expect(sh.getState().discoveredSenders).toHaveLength(1);
  });

  it('updates an existing sender in-place', () => {
    const { sh, discovery } = setup();
    const sender = makeDiscoveredSender({ deviceName: 'PC-1' });
    discovery.updateDiscoveredSender(sender);
    discovery.updateDiscoveredSender({ ...sender, deviceName: 'PC-1-Updated' });
    expect(sh.getState().discoveredSenders).toHaveLength(1);
    expect(sh.getState().discoveredSenders[0].deviceName).toBe('PC-1-Updated');
  });

  it('removes sender from list when isOffline=true', () => {
    const { sh, discovery } = setup();
    const sender = makeDiscoveredSender();
    discovery.updateDiscoveredSender(sender);
    discovery.updateDiscoveredSender({ ...sender, isOffline: true });
    expect(sh.getState().discoveredSenders).toHaveLength(0);
  });

  it('does nothing to an already-absent offline sender', () => {
    const { sh, discovery } = setup();
    discovery.updateDiscoveredSender(makeDiscoveredSender({ isOffline: true }));
    expect(sh.getState().discoveredSenders).toHaveLength(0);
  });

  it('syncs connectedSender when the matching sender is updated', () => {
    const { sh, discovery } = setup();
    const sender = makeDiscoveredSender({ deviceName: 'PC-1' });
    sh.setState({ connectedSender: sender });
    discovery.updateDiscoveredSender({ ...sender, deviceName: 'PC-1-Updated' });
    expect(sh.getState().connectedSender?.deviceName).toBe('PC-1-Updated');
  });

  it('leaves connectedSender unchanged when another sender updates', () => {
    const { sh, discovery } = setup();
    const connected = makeDiscoveredSender({ deviceId: 'A', deviceName: 'PC-A' });
    const other = makeDiscoveredSender({ deviceId: 'B', deviceName: 'PC-B' });
    sh.setState({ connectedSender: connected });
    discovery.updateDiscoveredSender(other);
    expect(sh.getState().connectedSender?.deviceId).toBe('A');
  });

  it('fires autoReconnectCallback when lastConnectedSender re-appears while Listening', () => {
    const { sh, discovery, reconnectCb } = setup();
    const sender = makeDiscoveredSender();
    sh.setState({
      status: Status.Listening,
      lastConnectedSender: sender,
    });
    discovery.updateDiscoveredSender(sender);
    expect(reconnectCb).toHaveBeenCalledTimes(1);
    // @ts-expect-error
    expect((reconnectCb.mock.calls[0] as [DiscoveredSender])[0].deviceId).toBe(
      sender.deviceId,
    );
  });

  it('does NOT fire autoReconnectCallback when status is not Listening', () => {
    const { sh, discovery, reconnectCb } = setup();
    const sender = makeDiscoveredSender();
    sh.setState({ status: Status.Connected, lastConnectedSender: sender });
    discovery.updateDiscoveredSender(sender);
    expect(reconnectCb).not.toHaveBeenCalled();
  });
});
