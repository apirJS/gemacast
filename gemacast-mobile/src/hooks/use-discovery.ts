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

/**
 * User-initiated rescan (pull-to-refresh).
 *
 * Drops network-discovered senders so departed/stale PCs clear immediately,
 * then re-arms the passive listeners (UDP presence + mDNS) so the list
 * repopulates with a fresh sweep. Kept out of the `startListening`/
 * `stopListening` wrappers on purpose: those mutate `status`
 * (Idle/Listening), which would knock a live stream back to "Scanning".
 * Re-arming discovery is independent of the audio session on the Rust side
 * (`stop_listening_for_senders` only aborts the discovery task), so this is
 * safe while connected or playing.
 *
 * Manually-added senders (`manual-*`) and the currently connected sender are
 * preserved — neither comes from discovery, so a rescan must not evict them.
 */
export async function refreshSenders(): Promise<Result<true, GemaCastError>> {
  const state = store.getState();
  const connectedId = state.connectedSender?.deviceId;
  const kept = state.discoveredSenders.filter(
    (sender) => sender.deviceId.startsWith('manual-') || sender.deviceId === connectedId,
  );
  store.getState().setDiscoveredSenders(kept);

  const activeStatuses: Status[] = [
    Status.Connecting,
    Status.Connected,
    Status.Playing,
    Status.Paused,
    Status.Reconnecting,
  ];
  if (!activeStatuses.includes(state.status)) {
    store.getState().setStatus(Status.Listening);
  }

  try {
    await tauriBridge.stopListeningForSenders();
    await tauriBridge.startListeningForSenders({
      deviceId: state.deviceInfo.deviceId,
      mode: state.settings.mode,
    });
    return ok(true);
  } catch (e) {
    const error = GemaCastError.failedToStartDiscovery(e);
    store.getState().displayError(error);
    return err(error);
  }
}

export function useDiscovery() {
  return { startListening, stopListening, refreshSenders };
}
