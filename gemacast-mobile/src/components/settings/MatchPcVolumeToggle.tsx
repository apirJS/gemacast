import { useSettings } from '../../hooks/use-settings';
import { useAppStore } from '../../stores/app-store';
import { Toggle } from '../shared/Toggle';

export function MatchPcVolumeToggle() {
  const { settings, update } = useSettings();
  const volumeSyncSupported = useAppStore(
    (s) => s.streamerCapabilities?.supportsVolumeSync ?? false,
  );

  return (
    <Toggle
      id="setting-match-pc-volume"
      checked={settings.matchPcVolume && volumeSyncSupported}
      onChange={(checked) => update({ matchPcVolume: checked })}
      disabled={!volumeSyncSupported}
    />
  );
}
