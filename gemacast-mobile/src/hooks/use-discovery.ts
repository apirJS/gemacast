import { useAppStore } from '../stores/app-store';
import { tauriBridge } from '../core/tauri-bridge';
import { GemaCastError } from '../core/error';
import { Status, type ConnectionMode, type Result } from '../core/types';
import { ok, err } from '../core/types';

const store = useAppStore;

export async function startListening(mode: ConnectionMode): Promise<Result<true, GemaCastError>> {
  store.getState().setLoading(true);
  try {
    const state = store.getState();
    await tauriBridge.startListeningForSenders({
      deviceId: state.deviceInfo.deviceId,
      mode,
    });
    store.getState().patch({ status: Status.Listening, isLoading: false });
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStartDiscovery(e);
    store.getState().displayError(error);
    store.getState().patch({ isLoading: false });
    return err(error);
  }
}

export async function stopListening(): Promise<Result<true, GemaCastError>> {
  try {
    await tauriBridge.stopListeningForSenders();
    store.getState().setStatus(Status.Idle);
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStopDiscovery(e);
    store.getState().displayError(error);
    return err(error);
  }
}

export function useDiscovery() {
  return { startListening, stopListening };
}
