import { useSettings } from '../../hooks/use-settings';
import { useAppStore } from '../../stores/app-store';
import { Toggle } from '../shared/Toggle';

export function ExclusiveToggle() {
  const { settings, update } = useSettings();
  const exclusiveSupported = useAppStore((s) => s.exclusiveSupported);

  return (
    <Toggle
      id="setting-exclusive-mode"
      checked={settings.exclusiveMode}
      onChange={(checked) => update({ exclusiveMode: checked })}
      disabled={!exclusiveSupported}
    />
  );
}
