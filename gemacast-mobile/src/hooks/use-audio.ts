import { useAppStore } from '../stores/app-store';
import { tauriBridge } from '../core/tauri-bridge';
import { GemaCastError } from '../core/error';
import { Status, type Result } from '../core/types';
import { ok, err } from '../core/types';

const store = useAppStore;

export async function startPlayback(): Promise<Result<true, GemaCastError>> {
  const { status } = store.getState();
  if (status === Status.Playing || status === Status.Connected) return ok(true);

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
  if (store.getState().status === Status.Paused) return ok(true);

  store.getState().setLoading(true);
  try {
    const state = store.getState();
    const sender = state.connectedSender;
    await tauriBridge.stopAudioPlayback({
      ip: sender ? sender.addr.split(':')[0] : null,
      deviceId: state.deviceInfo.deviceId,
    });
    // Transition to Paused — the session stays alive, only the Oboe stream
    // is silenced. connectedSender remains set.
    store.getState().patch({ status: Status.Paused, isLoading: false });
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStopPlayback(e);
    store.getState().patch({ error, isLoading: false });
    return err(error);
  }
}

export function updateAudioActive(isActive: boolean) {
  const state = store.getState();
  
  // If the user explicitly paused, ignore any audio activity telemetry
  // (both stale isActive: true packets and confirming isActive: false packets).
  // The state remains Paused until they explicitly resume via startPlayback.
  if (state.status === Status.Paused) return;

  if (
    state.status === Status.Playing ||
    state.status === Status.Connected
  ) {
    store.getState().setStatus(isActive ? Status.Playing : Status.Connected);
  }
}

export function useAudio() {
  return { startPlayback, stopPlayback, updateAudioActive };
}
