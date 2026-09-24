import { useAppStore } from '../stores/app-store';
import { tauriBridge, resolveBitrate } from '../core/tauri-bridge';
import { getPresetConfig } from '../core/presets';
import { Status, type AppSettings, type BitratePreset, type PresetId } from '../core/types';
import { useToastStore } from '../stores/toast-store';

export class SettingsController {
  async update(patch: Partial<AppSettings>): Promise<boolean> {
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
        patch.gainDb !== undefined ||
        patch.matchPcVolume !== undefined),
    );

    if (!needsRemoteApply) {
      state.updateSettings(patch);
      return true;
    }

    try {
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
        const bitrate = resolveBitrate(nextSettings.bitratePreset, nextSettings.customBitrateKbps);
        await tauriBridge.changeAudioBitrate({ ip, deviceId, bitrate });
      }

      if (patch.exclusiveMode !== undefined && connectedStreamer) {
        await tauriBridge.restartSession({ exclusiveMode: patch.exclusiveMode });
      }

      if (patch.gainDb !== undefined) {
        await tauriBridge.setAudioGain({ gainDb: patch.gainDb });
      }

      if (patch.matchPcVolume !== undefined) {
        await tauriBridge.setMatchPcVolume({ enabled: patch.matchPcVolume });
      }

      useAppStore.getState().updateSettings(patch);
      return true;
    } catch (error) {
      console.warn('Failed to apply settings', error);
      useToastStore.getState().show('warning', 'Setting was not applied');
      return false;
    }
  }

  setTheme(theme: AppSettings['theme']) {
    useAppStore.getState().updateSettings({ theme });
    document.documentElement.classList.toggle('dark', theme === 'dark');
    document.documentElement.classList.toggle('light', theme === 'light');
  }

  requiresNoBufferWarning(): boolean {
    try {
      return localStorage.getItem('gemacast_nobuffer_warning_dismissed') !== 'true';
    } catch {
      return true;
    }
  }

  dismissNoBufferWarning(): void {
    try {
      localStorage.setItem('gemacast_nobuffer_warning_dismissed', 'true');
    } catch {
      // The warning remains enabled when storage is unavailable.
    }
  }

  selectBufferPreset(preset: string): Promise<boolean> {
    const settings = useAppStore.getState().settings;
    if (preset.startsWith('saved-')) {
      const saved = settings.savedPresets[Number.parseInt(preset.replace('saved-', ''), 10)];
      if (!saved) return Promise.resolve(false);
      const config =
        saved.config.staticTargetMs == null ? { ...saved.config, staticTargetMs: 0 } : saved.config;
      return this.update({ bufferPreset: preset, customJitterConfig: config });
    }

    if (preset === 'custom') {
      return this.update({
        bufferPreset: 'custom',
        customJitterConfig: {
          minDepthMs: 25,
          comfortCapMs: 1000,
          peakDecayHalflifeMs: 0,
          resumeThresholdPct: 0.25,
          staticTargetMs: 0,
        },
      });
    }

    return this.update({ bufferPreset: preset as PresetId });
  }

  selectBitrate(preset: BitratePreset): Promise<boolean> {
    return this.update({ bitratePreset: preset });
  }

  applyCustomBitrate(value: string): Promise<boolean> {
    const bitrate = Number(value);
    if (!Number.isInteger(bitrate) || bitrate < 6 || bitrate > 512) {
      return Promise.resolve(false);
    }
    return this.update({ customBitrateKbps: bitrate, bitratePreset: 'custom' });
  }
}

export const settingsController = new SettingsController();

export function useSettingsController() {
  const settings = useAppStore((state) => state.settings);
  return { settings, update: settingsController.update.bind(settingsController) };
}
