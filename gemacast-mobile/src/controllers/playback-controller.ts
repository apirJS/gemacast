import { GemaCastError } from '../core/error';
import { err, ok, Status, type Result } from '../core/types';
import { tauriBridge } from '../core/tauri-bridge';
import { useAppStore } from '../stores/app-store';

export class PlaybackController {
  async start(): Promise<Result<true, GemaCastError>> {
    const currentStatus = useAppStore.getState().status;
    if (currentStatus === Status.Playing || currentStatus === Status.Connected) return ok(true);

    useAppStore.getState().setLoading(true);
    try {
      const state = useAppStore.getState();
      await tauriBridge.startAudioPlayback({
        ip: state.connectedStreamer?.addr.split(':')[0] ?? null,
        deviceId: state.deviceInfo.deviceId,
        deviceName: state.deviceInfo.deviceName,
      });
      const current = useAppStore.getState();
      current.patch(
        current.connectedStreamer
          ? { status: Status.Connected, isLoading: false }
          : { isLoading: false },
      );
      return ok(true);
    } catch (cause) {
      const error = GemaCastError.failedToStartPlayback(cause);
      useAppStore.getState().patch({ error, isLoading: false });
      return err(error);
    }
  }

  async stop(): Promise<Result<true, GemaCastError>> {
    if (useAppStore.getState().status === Status.Paused) return ok(true);

    useAppStore.getState().setLoading(true);
    try {
      const state = useAppStore.getState();
      await tauriBridge.stopAudioPlayback({
        ip: state.connectedStreamer?.addr.split(':')[0] ?? null,
        deviceId: state.deviceInfo.deviceId,
      });
      state.patch({ status: Status.Paused, isLoading: false });
      return ok(true);
    } catch (cause) {
      const error = GemaCastError.failedToStopPlayback(cause);
      useAppStore.getState().patch({ error, isLoading: false });
      return err(error);
    }
  }

  reportActivity(isActive: boolean): void {
    const state = useAppStore.getState();
    if (state.status === Status.Paused) return;
    if (state.status === Status.Playing || state.status === Status.Connected) {
      state.setStatus(isActive ? Status.Playing : Status.Connected);
    }
  }
}

export const playbackController = new PlaybackController();
