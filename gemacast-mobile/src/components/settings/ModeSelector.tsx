import { useSettings } from '../../hooks/use-settings';
import { useAppStore } from '../../stores/app-store';
import { SegmentedControl } from '../shared/SegmentedControl';
import { ConnectionMode } from '../../core/types';

export function ModeSelector() {
  const { settings, update } = useSettings();
  const modes = useAppStore((s) => s.availableModes);

  return (
    <div>
      <SegmentedControl
        name="conn-mode"
        value={settings.mode}
        onChange={(mode) => update({ mode })}
        options={[
          { value: ConnectionMode.Wifi, label: 'WiFi', disabled: !modes.wifi },
          { value: ConnectionMode.Usb, label: 'USB', disabled: !modes.usb },
          { value: ConnectionMode.Adb, label: 'ADB', disabled: !modes.adb },
        ]}
      />
    </div>
  );
}
