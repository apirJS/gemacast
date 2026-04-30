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
    sh.setState({ connectedSender: makeDiscoveredSender() });
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
