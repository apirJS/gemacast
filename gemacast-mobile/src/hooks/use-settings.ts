import { useAppStore } from '../stores/app-store';
import { tauriBridge, resolveBitrate } from '../core/tauri-bridge';
import { getPresetConfig } from '../core/presets';
import { Status, type AppSettings } from '../core/types';
import { useToastStore } from '../stores/toast-store';

export function useSettings() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  const update = (patch: Partial<AppSettings>): Promise<boolean> => {
    const state = useAppStore.getState();
    const nextSettings = { ...state.settings, ...patch };
    const connectedStreamer =
      (state.status === Status.Connected ||
        state.status === Status.Playing ||
        state.status === Status.Paused) &&
      state.connectedStreamer
        ? state.connectedStreamer
        : null;

    const needsRemoteApply = Boolean(
      connectedStreamer &&
      (patch.bufferPreset !== undefined ||
        patch.customJitterConfig !== undefined ||
        patch.bitratePreset !== undefined ||
        patch.customBitrateKbps !== undefined ||
        patch.exclusiveMode !== undefined ||
        patch.gainDb !== undefined),
    );

    // No stream is active, so there is nothing to acknowledge. Keep this
    // synchronous for ordinary settings/preset editing and persist directly.
    if (!needsRemoteApply) {
      updateSettings(patch);
      return Promise.resolve(true);
    }

    return (async () => {
      try {
        // Apply live settings first. Persistence follows only after the backend
        // acknowledges the operation, so a failed change leaves the previous
        // known-good setting intact.
        if (patch.bufferPreset !== undefined || patch.customJitterConfig !== undefined) {
          const activeConfig = getPresetConfig(
            nextSettings.bufferPreset,
            nextSettings.customJitterConfig,
          );
          await tauriBridge.updateJitterConfig({ jitterConfig: activeConfig });
        }

        if (
          (patch.bitratePreset !== undefined || patch.customBitrateKbps !== undefined) &&
          connectedStreamer
        ) {
          const ip = connectedStreamer.addr.split(':')[0];
          const deviceId = state.deviceInfo.deviceId;
          const bitrate = resolveBitrate(
            nextSettings.bitratePreset,
            nextSettings.customBitrateKbps,
          );
          await tauriBridge.changeAudioBitrate({ ip, deviceId, bitrate });
        }

        if (patch.exclusiveMode !== undefined && connectedStreamer) {
          await tauriBridge.restartSession({ exclusiveMode: patch.exclusiveMode });
        }

        if (patch.gainDb !== undefined) {
          await tauriBridge.setAudioGain({ gainDb: patch.gainDb });
        }

        updateSettings(patch);
        return true;
      } catch (error) {
        console.warn('Failed to apply settings', error);
        useToastStore.getState().show('warning', 'Setting was not applied');
        return false;
      }
    })();
  };

  return { settings, update };
}
