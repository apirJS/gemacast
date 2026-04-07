import { describe, it, expect, beforeEach } from 'bun:test';
import { StateHandler } from './StateHandler';
import {
  setupBrowserGlobals,
  makeDeviceInfo,
  makeDiscoveredSender,
  localStorageMock,
} from './testHelpers';
import { Status } from '../types';
import { GemaCastError } from '../error';

beforeEach(() => {
  setupBrowserGlobals();
});

describe('StateHandler — construction', () => {
  it('initialises with Idle status', () => {
    const sh = new StateHandler(makeDeviceInfo());
    expect(sh.getState().status).toBe(Status.Idle);
  });

  it('stores the provided deviceInfo', () => {
    const info = makeDeviceInfo({ deviceName: 'My Phone' });
    const sh = new StateHandler(info);
    expect(sh.getState().deviceInfo.deviceName).toBe('My Phone');
  });

  it('reflects navigator.onLine for initial isNetworkAvailable', () => {
    setupBrowserGlobals(false);
    const sh = new StateHandler(makeDeviceInfo());
    expect(sh.getState().isNetworkAvailable).toBe(false);
  });

  it('loads lastConnectedSender from localStorage if present', () => {
    const sender = makeDiscoveredSender();
    localStorageMock.setItem('gemacast_last_sender', JSON.stringify(sender));
    const sh = new StateHandler(makeDeviceInfo());
    expect(sh.getState().lastConnectedSender?.deviceId).toBe(sender.deviceId);
  });

  it('sets lastConnectedSender to null when localStorage is empty', () => {
    const sh = new StateHandler(makeDeviceInfo());
    expect(sh.getState().lastConnectedSender).toBeNull();
  });
});

describe('StateHandler — setState', () => {
  it('merges partial state correctly', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.setState({ status: Status.Listening, isLoading: true });
    expect(sh.getState().status).toBe(Status.Listening);
    expect(sh.getState().isLoading).toBe(true);
  });

  it('does not clobber unrelated fields', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.setState({ status: Status.Listening });
    expect(sh.getState().connectionHealth).toBe('ok');
    expect(sh.getState().reconnectAttempts).toBe(0);
  });

  it('notifies subscribers on every call', () => {
    const sh = new StateHandler(makeDeviceInfo());
    const calls: string[] = [];
    sh.subscribe((s) => {
      calls.push(s.status);
    });
    sh.setState({ status: Status.Listening });
    sh.setState({ status: Status.Connected });
    // First call is the immediate notification on subscribe, then two updates.
    expect(calls).toEqual([Status.Idle, Status.Listening, Status.Connected]);
  });

  it('unsubscribe stops future notifications', () => {
    const sh = new StateHandler(makeDeviceInfo());
    const calls: number[] = [];
    const unsub = sh.subscribe(() => {
      calls.push(1);
    });
    unsub();
    sh.setState({ status: Status.Listening });
    expect(calls).toHaveLength(1); // only the initial call
  });
});

describe('StateHandler — error helpers', () => {
  it('displayError wraps a plain string in GemaCastError', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.displayError('something went wrong');
    expect(sh.getState().error).toBeInstanceOf(GemaCastError);
  });

  it('displayError passes a GemaCastError through as-is', () => {
    const sh = new StateHandler(makeDeviceInfo());
    const err = GemaCastError.senderTimeout();
    sh.displayError(err);
    expect(sh.getState().error).toBe(err);
  });

  it('dismissError clears the error', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.displayError('oops');
    sh.dismissError();
    expect(sh.getState().error).toBeNull();
  });
});

describe('StateHandler — setConnectionHealth', () => {
  it('updates connectionHealth', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.setConnectionHealth('degraded');
    expect(sh.getState().connectionHealth).toBe('degraded');
  });
});

describe('StateHandler — updateLatencyInfo', () => {
  it('stores all four latency fields', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.updateLatencyInfo(42, 38, 60, 20);
    const { latency } = sh.getState();
    expect(latency.current).toBe(42);
    expect(latency.avg).toBe(38);
    expect(latency.max).toBe(60);
    expect(latency.min).toBe(20);
  });

  it('accepts nulls (reset)', () => {
    const sh = new StateHandler(makeDeviceInfo());
    sh.updateLatencyInfo(1, 2, 3, 4);
    sh.updateLatencyInfo(null, null, null, null);
    const { latency } = sh.getState();
    expect(latency.current).toBeNull();
  });
});

describe('StateHandler — static localStorage helpers', () => {
  it('saveLastSender persists and loadLastSender retrieves', () => {
    const sender = makeDiscoveredSender();
    StateHandler.saveLastSender(sender);
    expect(StateHandler.loadLastSender()?.deviceId).toBe(sender.deviceId);
  });

  it('saveLastSender(null) removes the key', () => {
    const sender = makeDiscoveredSender();
    StateHandler.saveLastSender(sender);
    StateHandler.saveLastSender(null);
    expect(StateHandler.loadLastSender()).toBeNull();
  });

  it('loadLastSender returns null on corrupt JSON', () => {
    localStorageMock.setItem('gemacast_last_sender', '{bad json');
    expect(StateHandler.loadLastSender()).toBeNull();
  });
});
