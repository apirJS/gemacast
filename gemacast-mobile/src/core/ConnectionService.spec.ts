import { describe, it, expect, beforeEach, mock } from 'bun:test';
import {
  setupBrowserGlobals,
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredSender,
  localStorageMock,
} from './testHelpers';
import { StateHandler } from './StateHandler';
import { ConnectionService } from './ConnectionService';
import { Status } from '../types';
import { GemaCastError } from '../error';

function setup(
  handlers: Record<string, unknown> = {},
  opts: { onLine?: boolean } = {},
) {
  setupBrowserGlobals(opts.onLine ?? true);
  setupInvokeMock(handlers);
  const sh = new StateHandler(makeDeviceInfo());
  const audioResumer = mock(async () => {});
  const conn = new ConnectionService(sh, audioResumer);
  return { sh, conn, audioResumer };
}

beforeEach(() => {
  setupBrowserGlobals();
  localStorageMock.clear();
});

describe('ConnectionService — connectToSender', () => {
  it('transitions through Connecting → Connected on success', async () => {
    const { sh, conn } = setup({ connect_to_sender: undefined });
    const sender = makeDiscoveredSender();
    const result = await conn.connectToSender(sender);
    expect(result.ok).toBe(true);
    expect(sh.getState().status).toBe(Status.Connected);
    expect(sh.getState().connectedSender?.deviceId).toBe(sender.deviceId);
    expect(sh.getState().isLoading).toBe(false);
  });

  it('invokes connect_to_sender with ip/deviceId/deviceName', async () => {
    const { conn } = setup({ connect_to_sender: undefined });
    await conn.connectToSender(makeDiscoveredSender({ addr: '10.0.0.1:9000' }));
    expect(invokeCalls[0]?.cmd).toBe('connect_to_sender');
    expect((invokeCalls[0]?.args as Record<string, unknown>).ip).toBe(
      '10.0.0.1',
    );
  });

  it('saves lastConnectedSender to localStorage', async () => {
    const { conn } = setup({ connect_to_sender: undefined });
    const sender = makeDiscoveredSender();
    await conn.connectToSender(sender);
    const saved = StateHandler.loadLastSender();
    expect(saved?.deviceId).toBe(sender.deviceId);
  });

  it('returns err and reverts to Listening on IPC failure', async () => {
    setupInvokeMock({
      connect_to_sender: () => {
        throw new Error('refused');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    const conn = new ConnectionService(
      sh,
      mock(async () => {}),
    );
    const result = await conn.connectToSender(makeDiscoveredSender());
    expect(result.ok).toBe(false);
    expect(sh.getState().status).toBe(Status.Listening);
    expect(sh.getState().error).toBeInstanceOf(GemaCastError);
  });

  it('resets reconnectAttempts to 0 on connect', async () => {
    const { sh, conn } = setup({ connect_to_sender: undefined });
    sh.setState({ reconnectAttempts: 3 });
    await conn.connectToSender(makeDiscoveredSender());
    expect(sh.getState().reconnectAttempts).toBe(0);
  });
});

describe('ConnectionService — disconnect', () => {
  it('transitions to Listening and clears connectedSender', async () => {
    const { sh, conn } = setup({ disconnect_from_sender: undefined });
    const sender = makeDiscoveredSender();
    sh.setState({ connectedSender: sender, status: Status.Connected });
    const result = await conn.disconnect();
    expect(result.ok).toBe(true);
    expect(sh.getState().connectedSender).toBeNull();
    expect(sh.getState().status).toBe(Status.Listening);
  });

  it('invokes disconnect_from_sender IPC', async () => {
    const { sh, conn } = setup({ disconnect_from_sender: undefined });
    sh.setState({
      connectedSender: makeDiscoveredSender({ addr: '10.0.0.2:9000' }),
    });
    await conn.disconnect();
    expect(invokeCalls.some((c) => c.cmd === 'disconnect_from_sender')).toBe(
      true,
    );
  });

  it('clears lastConnectedSender from localStorage', async () => {
    const { sh, conn } = setup({ disconnect_from_sender: undefined });
    const sender = makeDiscoveredSender();
    StateHandler.saveLastSender(sender);
    sh.setState({ connectedSender: sender });
    await conn.disconnect();
    expect(StateHandler.loadLastSender()).toBeNull();
  });

  it('still succeeds (ok:true) when no sender is connected', async () => {
    const { conn } = setup({ disconnect_from_sender: undefined });
    const result = await conn.disconnect();
    expect(result.ok).toBe(true);
  });

  it('still transitions to Listening even when IPC throws', async () => {
    setupInvokeMock({
      disconnect_from_sender: () => {
        throw new Error('net');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    sh.setState({ connectedSender: makeDiscoveredSender() });
    const conn = new ConnectionService(
      sh,
      mock(async () => {}),
    );
    await conn.disconnect();
    expect(sh.getState().status).toBe(Status.Listening);
  });

  it('resets latency to all-null', async () => {
    const { sh, conn } = setup({ disconnect_from_sender: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender() });
    sh.updateLatencyInfo(10, 12, 20, 5);
    await conn.disconnect();
    const { latency } = sh.getState();
    expect(latency.current).toBeNull();
    expect(latency.avg).toBeNull();
  });
});

describe('ConnectionService — handleSenderTimeout', () => {
  it('removes the timed-out sender from the list', () => {
    const { sh, conn } = setup();
    const sender = makeDiscoveredSender({ deviceId: 'gone' });
    sh.setState({ discoveredSenders: [sender] });
    conn.handleSenderTimeout('gone');
    expect(sh.getState().discoveredSenders).toHaveLength(0);
  });

  it('enters Reconnecting status when timeout is the connected sender', () => {
    const { sh, conn } = setup();
    const sender = makeDiscoveredSender({ deviceId: 'pc-1' });
    sh.setState({
      connectedSender: sender,
      discoveredSenders: [sender],
      status: Status.Connected,
    });
    conn.handleSenderTimeout('pc-1');
    expect(sh.getState().status).toBe(Status.Reconnecting);
    expect(sh.getState().connectedSender).toBeNull();
  });

  it('sets a senderTimeout error', () => {
    const { sh, conn } = setup();
    const sender = makeDiscoveredSender({ deviceId: 'pc-1' });
    sh.setState({ connectedSender: sender, discoveredSenders: [sender] });
    conn.handleSenderTimeout('pc-1');
    // @ts-expect-error
    expect(sh.getState().error?.code).toBe('NETWORK_SENDER_TIMEOUT');
  });

  it('does not change status when a different sender times out', () => {
    const { sh, conn } = setup();
    const connected = makeDiscoveredSender({ deviceId: 'active' });
    sh.setState({ connectedSender: connected, status: Status.Playing });
    conn.handleSenderTimeout('other-device');
    expect(sh.getState().status).toBe(Status.Playing);
  });
});

describe('ConnectionService — handleForceDisconnect', () => {
  it('clears connectedSender and moves to Listening', () => {
    const { sh, conn } = setup();
    sh.setState({
      connectedSender: makeDiscoveredSender(),
      status: Status.Playing,
    });
    conn.handleForceDisconnect();
    expect(sh.getState().connectedSender).toBeNull();
    expect(sh.getState().status).toBe(Status.Listening);
  });

  it('forgets lastConnectedSender when forgetSender=true (default)', () => {
    const { sh, conn } = setup();
    const sender = makeDiscoveredSender();
    StateHandler.saveLastSender(sender);
    sh.setState({ lastConnectedSender: sender });
    conn.handleForceDisconnect(true);
    expect(sh.getState().lastConnectedSender).toBeNull();
    expect(StateHandler.loadLastSender()).toBeNull();
  });

  it('retains lastConnectedSender when forgetSender=false', () => {
    const { sh, conn } = setup();
    const sender = makeDiscoveredSender();
    sh.setState({ connectedSender: sender, lastConnectedSender: sender });
    conn.handleForceDisconnect(false);
    expect(sh.getState().lastConnectedSender?.deviceId).toBe(sender.deviceId);
  });
});
