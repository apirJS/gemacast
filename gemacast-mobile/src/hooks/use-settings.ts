import { useCallback } from 'react';
import { useAppStore } from '../stores/app-store';
import { tauriBridge, resolveBitrate } from '../core/tauri-bridge';
import { getPresetConfig } from '../core/presets';
import { Status, type AppSettings } from '../core/types';
import { connectToSender, disconnect } from './use-connection';

export function useSettings() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  const update = useCallback(
    (patch: Partial<AppSettings>) => {
      updateSettings(patch);

      // If buffer settings changed, notify the backend live
      if (patch.bufferPreset !== undefined || patch.customJitterConfig !== undefined) {
        const nextSettings = { ...useAppStore.getState().settings, ...patch };
        const activeConfig = getPresetConfig(
          nextSettings.bufferPreset,
          nextSettings.customJitterConfig
        );
        tauriBridge.updateJitterConfig({ jitterConfig: activeConfig }).catch((e) => {
          console.warn('Failed to update live jitter config', e);
        });
      }

      // If bitrate settings changed, notify the backend live
      if (patch.bitratePreset !== undefined || patch.customBitrateKbps !== undefined) {
        const state = useAppStore.getState();
        const nextSettings = { ...state.settings, ...patch };
        if ((state.status === Status.Connected || state.status === Status.Playing) && state.connectedSender) {
          const ip = state.connectedSender.addr.split(':')[0];
          const deviceId = state.deviceInfo.deviceId;
          const bitrate = resolveBitrate(nextSettings.bitratePreset, nextSettings.customBitrateKbps);
          tauriBridge.changeAudioBitrate({ ip, deviceId, bitrate }).catch((e) => {
             console.warn('Failed to change audio bitrate', e);
          });
        }
      }

      // If exclusive mode changed while connected, reconnect to apply new Oboe SharingMode
      if (patch.exclusiveMode !== undefined) {
        const state = useAppStore.getState();
        if (
          (state.status === Status.Connected || state.status === Status.Playing) &&
          state.connectedSender
        ) {
          const sender = state.connectedSender;
          disconnect(false).then(() => connectToSender(sender)).catch((e) => {
            console.warn('Failed to reconnect after exclusive mode change', e);
          });
        }
      }
    },
    [updateSettings],
  );

  return { settings, update };
}
