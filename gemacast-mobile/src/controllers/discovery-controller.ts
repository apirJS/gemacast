import { GemaCastError } from '../core/error';
import { err, ok, Status, type ConnectionMode, type Result } from '../core/types';
import { tauriBridge } from '../core/tauri-bridge';
import { useAppStore } from '../stores/app-store';

const ACTIVE_SESSION_STATUSES = [
  Status.Connecting,
  Status.Connected,
  Status.Playing,
  Status.Paused,
  Status.Reconnecting,
];

export class DiscoveryController {
  async start(mode: ConnectionMode): Promise<Result<true, GemaCastError>> {
    useAppStore.getState().setLoading(true);
    try {
      const state = useAppStore.getState();
      await tauriBridge.startListeningForStreamers({
        deviceId: state.deviceInfo.deviceId,
        mode,
      });
      useAppStore.getState().patch({ status: Status.Listening, isLoading: false });
      return ok(true);
    } catch (cause) {
      const error = GemaCastError.failedToStartDiscovery(cause);
      useAppStore.getState().displayError(error);
      useAppStore.getState().setLoading(false);
      return err(error);
    }
  }

  async stop(): Promise<Result<true, GemaCastError>> {
    try {
      await tauriBridge.stopListeningForStreamers();
      useAppStore.getState().setStatus(Status.Idle);
      return ok(true);
    } catch (cause) {
      const error = GemaCastError.failedToStopDiscovery(cause);
      useAppStore.getState().displayError(error);
      return err(error);
    }
  }

  async refresh(): Promise<Result<true, GemaCastError>> {
    const state = useAppStore.getState();
    const connectedId = state.connectedStreamer?.deviceId;
    const retainedStreamers = state.discoveredStreamers.filter(
      (streamer) => streamer.deviceId.startsWith('manual-') || streamer.deviceId === connectedId,
    );
    state.setDiscoveredStreamers(retainedStreamers);
    if (!ACTIVE_SESSION_STATUSES.includes(state.status)) state.setStatus(Status.Listening);

    try {
      await tauriBridge.stopListeningForStreamers();
      await tauriBridge.startListeningForStreamers({
        deviceId: state.deviceInfo.deviceId,
        mode: state.settings.mode,
      });
      return ok(true);
    } catch (cause) {
      const error = GemaCastError.failedToStartDiscovery(cause);
      useAppStore.getState().displayError(error);
      return err(error);
    }
  }
}

export const discoveryController = new DiscoveryController();
