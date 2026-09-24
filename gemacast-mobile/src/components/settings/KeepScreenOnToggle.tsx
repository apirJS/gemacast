import { useSettingsController } from '../../controllers';
import { Toggle } from '../shared/Toggle';

export function KeepScreenOnToggle() {
  const { settings, update } = useSettingsController();

  return (
    <Toggle
      id="setting-keep-screen-on"
      checked={settings.keepScreenOn}
      onChange={(checked) => update({ keepScreenOn: checked })}
    />
  );
}
