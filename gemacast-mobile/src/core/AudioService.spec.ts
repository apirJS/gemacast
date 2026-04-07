import { describe, it, expect, beforeEach } from 'bun:test';
import {
  setupBrowserGlobals,
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredSender,
} from './testHelpers';
import { StateHandler } from './StateHandler';
import { AudioService } from './AudioService';
import { Status } from '../types';
import { GemaCastError } from '../error';

function setup(handlers: Record<string, unknown> = {}) {
  setupInvokeMock(handlers);
  const sh = new StateHandler(makeDeviceInfo());
  const audio = new AudioService(sh);
  return { sh, audio };
}

beforeEach(() => {
  setupBrowserGlobals();
});

describe('AudioService — startAudioPlayback', () => {
  it('sets isLoading then Playing on success', async () => {
    const { sh, audio } = setup({ start_audio_playback: undefined });
    const result = await audio.startAudioPlayback();
    expect(result.ok).toBe(true);
    expect(sh.getState().status).toBe(Status.Playing);
    expect(sh.getState().isLoading).toBe(false);
  });

  it('invokes start_audio_playback IPC', async () => {
    const { audio } = setup({ start_audio_playback: undefined });
    await audio.startAudioPlayback();
    expect(invokeCalls[0]?.cmd).toBe('start_audio_playback');
  });

  it('returns err and stores error on IPC failure', async () => {
    setupInvokeMock({
      start_audio_playback: () => {
        throw new Error('boom');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    const audio = new AudioService(sh);
    const result = await audio.startAudioPlayback();
    expect(result.ok).toBe(false);
    expect(sh.getState().error).toBeInstanceOf(GemaCastError);
    expect(sh.getState().isLoading).toBe(false);
  });
});

describe('AudioService — stopAudioPlayback', () => {
  it('transitions to Connected status on success', async () => {
    const { sh, audio } = setup({ stop_audio_playback: undefined });
    sh.setState({ status: Status.Playing });
    const result = await audio.stopAudioPlayback();
    expect(result.ok).toBe(true);
    expect(sh.getState().status).toBe(Status.Connected);
  });

  it('stores error on IPC failure', async () => {
    setupInvokeMock({
      stop_audio_playback: () => {
        throw new Error('fail');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    const audio = new AudioService(sh);
    const result = await audio.stopAudioPlayback();
    expect(result.ok).toBe(false);
    expect(sh.getState().error).toBeInstanceOf(GemaCastError);
  });
});

describe('AudioService — updateAudioActive', () => {
  it('switches Playing → Connected when audio goes silent', () => {
    const { sh, audio } = setup();
    sh.setState({ status: Status.Playing });
    audio.updateAudioActive(false);
    expect(sh.getState().status).toBe(Status.Connected);
  });

  it('switches Connected → Playing when audio becomes active', () => {
    const { sh, audio } = setup();
    sh.setState({ status: Status.Connected });
    audio.updateAudioActive(true);
    expect(sh.getState().status).toBe(Status.Playing);
  });

  it('does nothing when status is Idle', () => {
    const { sh, audio } = setup();
    sh.setState({ status: Status.Idle });
    audio.updateAudioActive(true);
    expect(sh.getState().status).toBe(Status.Idle);
  });
});

describe('AudioService — setRemoteVolume', () => {
  it('does nothing if no sender is connected', async () => {
    const { audio } = setup();
    await audio.setRemoteVolume(0.5); // no throw, no IPC
    expect(invokeCalls).toHaveLength(0);
  });

  it('optimistically updates volume in state', async () => {
    const { sh, audio } = setup({ set_remote_system_volume: undefined });
    const sender = makeDiscoveredSender();
    sh.setState({ connectedSender: sender });
    await audio.setRemoteVolume(0.6);
    expect(sh.getState().connectedSender?.volume).toBe(0.6);
  });

  it('clamps values above 1 to 1', async () => {
    const { sh, audio } = setup({ set_remote_system_volume: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender() });
    await audio.setRemoteVolume(1.5);
    expect(sh.getState().connectedSender?.volume).toBe(1);
  });

  it('clamps values below 0 to 0 and marks isMuted', async () => {
    const { sh, audio } = setup({ set_remote_system_volume: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender() });
    await audio.setRemoteVolume(-0.1);
    expect(sh.getState().connectedSender?.volume).toBe(0);
    expect(sh.getState().connectedSender?.isMuted).toBe(true);
  });

  it('invokes set_remote_system_volume with correct args', async () => {
    const { sh, audio } = setup({ set_remote_system_volume: undefined });
    const sender = makeDiscoveredSender({
      addr: '10.0.0.5:9000',
      deviceId: 'dev-1',
    });
    sh.setState({ connectedSender: sender });
    await audio.setRemoteVolume(0.8);
    expect(invokeCalls[0]?.cmd).toBe('set_remote_system_volume');
    expect((invokeCalls[0]?.args as Record<string, unknown>).ip).toBe(
      '10.0.0.5',
    );
    expect((invokeCalls[0]?.args as Record<string, unknown>).level).toBe(0.8);
  });

  it('does not throw when IPC fails', async () => {
    setupInvokeMock({
      set_remote_system_volume: () => {
        throw new Error('net err');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    sh.setState({ connectedSender: makeDiscoveredSender() });
    const audio = new AudioService(sh);
    await expect(audio.setRemoteVolume(0.5)).resolves.toBeUndefined();
  });
});

describe('AudioService — toggleRemoteMute', () => {
  it('does nothing if no sender is connected', async () => {
    const { audio } = setup();
    await audio.toggleRemoteMute();
    expect(invokeCalls).toHaveLength(0);
  });

  it('flips isMuted from false to true', async () => {
    const { sh, audio } = setup({ set_remote_system_mute: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender({ isMuted: false }) });
    await audio.toggleRemoteMute();
    expect(sh.getState().connectedSender?.isMuted).toBe(true);
  });

  it('flips isMuted from true to false', async () => {
    const { sh, audio } = setup({ set_remote_system_mute: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender({ isMuted: true }) });
    await audio.toggleRemoteMute();
    expect(sh.getState().connectedSender?.isMuted).toBe(false);
  });

  it('treats undefined isMuted as false (so first toggle → true)', async () => {
    const { sh, audio } = setup({ set_remote_system_mute: undefined });
    const sender = makeDiscoveredSender();
    delete (sender as { isMuted?: boolean }).isMuted;
    sh.setState({ connectedSender: sender });
    await audio.toggleRemoteMute();
    expect(sh.getState().connectedSender?.isMuted).toBe(true);
  });

  it('invokes set_remote_system_mute with correct muted value', async () => {
    const { sh, audio } = setup({ set_remote_system_mute: undefined });
    sh.setState({ connectedSender: makeDiscoveredSender({ isMuted: false }) });
    await audio.toggleRemoteMute();
    expect(invokeCalls[0]?.cmd).toBe('set_remote_system_mute');
    expect((invokeCalls[0]?.args as Record<string, unknown>).muted).toBe(true);
  });

  it('does not throw when IPC fails', async () => {
    setupInvokeMock({
      set_remote_system_mute: () => {
        throw new Error('net');
      },
    });
    const sh = new StateHandler(makeDeviceInfo());
    sh.setState({ connectedSender: makeDiscoveredSender() });
    const audio = new AudioService(sh);
    await expect(audio.toggleRemoteMute()).resolves.toBeUndefined();
  });
});
