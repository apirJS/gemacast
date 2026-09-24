import { useSettingsController } from '../../controllers';
import { Toggle } from '../shared/Toggle';

export function AutoReconnectToggle() {
  const { settings, update } = useSettingsController();

  return (
    <Toggle
      id="setting-auto-reconnect"
      checked={settings.autoReconnect}
      onChange={(checked) => update({ autoReconnect: checked })}
    />
  );
}
