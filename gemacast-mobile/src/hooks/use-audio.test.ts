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
  it('is a no-op when already Connected (echo prevention)', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      status: Status.Connected,
    });
    const result = await startPlayback();
    expect(result.ok).toBe(true);
    // No IPC should fire — the guard short-circuits
    expect(invokeCalls.some((c) => c.cmd === 'start_audio_playback')).toBe(false);
    expect(useAppStore.getState().status).toBe(Status.Connected);
  });

  it('resumes from Paused state', async () => {
    const sender = makeDiscoveredSender();
    useAppStore.getState().patch({
      connectedSender: sender,
      status: Status.Paused,
    });
    const result = await startPlayback();
    expect(result.ok).toBe(true);
    expect(invokeCalls.some((c) => c.cmd === 'start_audio_playback')).toBe(true);
    // connectedSender should remain set (no disconnect occurred)
    expect(useAppStore.getState().connectedSender).not.toBeNull();
    expect(useAppStore.getState().status).toBe(Status.Connected);
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
  it('transitions to Paused (not Connected)', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Playing,
    });
    const result = await stopPlayback();
    expect(result.ok).toBe(true);
    expect(useAppStore.getState().status).toBe(Status.Paused);
    // connectedSender should remain set
    expect(useAppStore.getState().connectedSender).not.toBeNull();
  });

  it('does not invoke disconnect_from_sender', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Playing,
    });
    await stopPlayback();
    expect(invokeCalls.some((c) => c.cmd === 'disconnect_from_sender')).toBe(false);
  });

  it('is a no-op when already Paused (echo prevention)', async () => {
    useAppStore.getState().patch({
      connectedSender: makeDiscoveredSender(),
      status: Status.Paused,
    });
    invokeCalls.length = 0;
    const result = await stopPlayback();
    expect(result.ok).toBe(true);
    // No IPC should fire — the guard short-circuits
    expect(invokeCalls.some((c) => c.cmd === 'stop_audio_playback')).toBe(false);
    expect(useAppStore.getState().status).toBe(Status.Paused);
    expect(useAppStore.getState().isLoading).toBe(false);
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

  it('ignores telemetry completely when Paused (prevents UI flicker)', () => {
    useAppStore.getState().setStatus(Status.Paused);
    updateAudioActive(true); // Stale telemetry from before pause
    expect(useAppStore.getState().status).toBe(Status.Paused);
    
    updateAudioActive(false); // Confirmation telemetry after pause
    expect(useAppStore.getState().status).toBe(Status.Paused);
  });

  it('ignores when not in Connected/Playing/Paused state', () => {
    useAppStore.getState().setStatus(Status.Listening);
    updateAudioActive(true);
    expect(useAppStore.getState().status).toBe(Status.Listening);
  });
});
