import { useSettings } from '../../hooks/use-settings';
import { Toggle } from '../shared/Toggle';

export function AutoReconnectToggle() {
  const { settings, update } = useSettings();

  return (
    <Toggle
      id="setting-auto-reconnect"
      checked={settings.autoReconnect}
      onChange={(checked) => update({ autoReconnect: checked })}
    />
  );
}
