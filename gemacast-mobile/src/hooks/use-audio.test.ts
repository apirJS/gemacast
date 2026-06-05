import { describe, it, expect, beforeEach } from 'bun:test';
import { setupInvokeMock, invokeCalls, makeDeviceInfo, makeDiscoveredSender } from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { Status } from '../core/types';
import { startPlayback, stopPlayback, updateAudioActive } from './use-audio';

beforeEach(() => {
  setupInvokeMock({
    start_audio_playback: undefined,
    stop_audio_playback: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
});

describe('startPlayback', () => {
  it('invokes start_audio_playback and stays Connected', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      status: Status.Connected,
    });
    const result = await startPlayback();
    expect(result.ok).toBe(true);
    expect(invokeCalls.some((c) => c.cmd === 'start_audio_playback')).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Connected);
    expect(useAppStore.getState().isLoading).toBe(false);
  });

  it('returns err on IPC failure', async () => {
    setupInvokeMock({
      start_audio_playback: () => { throw new Error('no device'); },
    });
    const result = await startPlayback();
    expect(result.ok).toBe(false);
    expect(useAppStore.getState().error).not.toBeNull();
  });
});

describe('stopPlayback', () => {
  it('transitions back to Connected', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Playing,
    });
    const result = await stopPlayback();
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });
});

describe('updateAudioActive', () => {
  it('transitions to Playing when active', () => {
    useAppStore.getState().setStatus(Status.Connected);
    updateAudioActive(true);
    expect(useAppStore.getState().status).toBe(Status.Playing);
  });

  it('transitions to Connected when inactive', () => {
    useAppStore.getState().setStatus(Status.Playing);
    updateAudioActive(false);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });

  it('ignores when not in Connected/Playing state', () => {
    useAppStore.getState().setStatus(Status.Listening);
    updateAudioActive(true);
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });
});
