import { useSettingsController } from '../../controllers';
import { useAppStore } from '../../stores/app-store';
import { Toggle } from '../shared/Toggle';

export function ExclusiveToggle() {
  const { settings, update } = useSettingsController();
  const exclusiveSupported = useAppStore((s) => s.exclusiveSupported);

  return (
    <Toggle
      id="setting-exclusive-mode"
      checked={settings.exclusiveMode && exclusiveSupported}
      onChange={(checked) => update({ exclusiveMode: checked })}
      disabled={!exclusiveSupported}
    />
  );
}
