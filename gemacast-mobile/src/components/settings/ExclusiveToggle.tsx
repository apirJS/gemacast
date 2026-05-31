import { useSettings } from '../../hooks/use-settings';
import { Toggle } from '../shared/Toggle';

export function ExclusiveToggle() {
  const { settings, update } = useSettings();

  return (
    <Toggle
      id="setting-exclusive-mode"
      checked={settings.exclusiveMode}
      onChange={(checked) => update({ exclusiveMode: checked })}
    />
  );
}
