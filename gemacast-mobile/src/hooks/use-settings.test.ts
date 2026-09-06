import { describe, it, expect, beforeEach } from 'bun:test';
import { renderHook, cleanup } from '@testing-library/react';
import {
  setupInvokeMock,
  invokeCalls,
  makeDeviceInfo,
  makeDiscoveredStreamer,
} from '../__tests__/setup';
import { useAppStore } from '../stores/app-store';
import { Status } from '../core/types';
import { useSettings } from './use-settings';

beforeEach(() => {
  cleanup();
  localStorage.clear();
  setupInvokeMock({
    set_match_pc_volume: undefined,
    set_audio_gain: undefined,
  });
  useAppStore.getState().init(makeDeviceInfo());
});

function connected() {
  useAppStore.getState().patch({
    connectedStreamer: makeDiscoveredStreamer(),
    status: Status.Connected,
  });
}

describe('useSettings matchPcVolume', () => {
  it('stores the choice without an IPC call while nothing is connected', async () => {
    const { result } = renderHook(() => useSettings());

    await result.current.update({ matchPcVolume: true });

    expect(useAppStore.getState().settings.matchPcVolume).toBe(true);
    expect(invokeCalls.some((c) => c.cmd === 'set_match_pc_volume')).toBe(false);
  });

  it('pushes the choice to the session while connected', async () => {
    connected();
    const { result } = renderHook(() => useSettings());

    await result.current.update({ matchPcVolume: true });

    const call = invokeCalls.find((c) => c.cmd === 'set_match_pc_volume');
    expect(call?.args).toEqual({ enabled: true });
    expect(useAppStore.getState().settings.matchPcVolume).toBe(true);
  });

  it('leaves the setting untouched when the session rejects it', async () => {
    setupInvokeMock({
      set_match_pc_volume: () => {
        throw new Error('denied');
      },
    });
    connected();
    const { result } = renderHook(() => useSettings());

    const applied = await result.current.update({ matchPcVolume: true });

    expect(applied).toBe(false);
    expect(useAppStore.getState().settings.matchPcVolume).toBe(false);
  });
});
