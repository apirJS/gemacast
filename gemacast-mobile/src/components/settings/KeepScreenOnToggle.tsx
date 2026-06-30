import { useSettings } from '../../hooks/use-settings';
import { Toggle } from '../shared/Toggle';

export function KeepScreenOnToggle() {
  const { settings, update } = useSettings();

  return (
    <Toggle
      id="setting-keep-screen-on"
      checked={settings.keepScreenOn}
      onChange={(checked) => update({ keepScreenOn: checked })}
    />
  );
}
