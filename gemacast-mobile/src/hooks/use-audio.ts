import { useAppStore } from '../stores/app-store';
import { tauriBridge } from '../core/tauri-bridge';
import { GemaCastError } from '../core/error';
import { Status, type Result } from '../core/types';
import { ok, err } from '../core/types';

const store = useAppStore;

export async function startPlayback(): Promise<Result<true, GemaCastError>> {
  store.getState().setLoading(true);
  try {
    const state = store.getState();
    const sender = state.connectedSender;
    await tauriBridge.startAudioPlayback({
      ip: sender ? sender.addr.split(':')[0] : null,
      deviceId: state.deviceInfo.deviceId,
      deviceName: state.deviceInfo.deviceName,
    });
    const current = store.getState();
    if (current.connectedSender) {
      store.getState().patch({ status: Status.Connected, isLoading: false });
    } else {
      store.getState().setLoading(false);
    }
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStartPlayback(e);
    store.getState().patch({ error, isLoading: false });
    return err(error);
  }
}

export async function stopPlayback(): Promise<Result<true, GemaCastError>> {
  store.getState().setLoading(true);
  try {
    const state = store.getState();
    const sender = state.connectedSender;
    await tauriBridge.stopAudioPlayback({
      ip: sender ? sender.addr.split(':')[0] : null,
      deviceId: state.deviceInfo.deviceId,
    });
    store.getState().patch({ status: Status.Connected, isLoading: false });
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStopPlayback(e);
    store.getState().patch({ error, isLoading: false });
    return err(error);
  }
}

export function updateAudioActive(isActive: boolean) {
  const state = store.getState();
  if (state.status === Status.Playing || state.status === Status.Connected) {
    store.getState().setStatus(isActive ? Status.Playing : Status.Connected);
  }
}

export function useAudio() {
  return { startPlayback, stopPlayback, updateAudioActive };
}
