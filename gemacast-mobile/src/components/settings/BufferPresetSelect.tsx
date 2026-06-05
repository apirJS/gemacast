import { useMemo } from 'react';
import { useSettings } from '../../hooks/use-settings';
import { CustomSelect, type SelectOption } from '../shared/CustomSelect';
import { JITTER_PRESETS } from '../../core/presets';
import type { PresetId } from '../../core/types';

export function BufferPresetSelect() {
  const { settings, update } = useSettings();

  const options: SelectOption<string>[] = useMemo(() => {
    const builtIn = JITTER_PRESETS.map((p) => ({
      value: p.id,
      label: p.name,
      description: p.description,
    }));

    const saved = settings.savedPresets.map((sp, i) => ({
      value: `saved-${i}`,
      label: sp.name,
      description: 'User-saved preset',
    }));

    return [...builtIn, ...saved];
  }, [settings.savedPresets]);

  const handleChange = (value: string) => {
    if (value.startsWith('saved-')) {
      const idx = parseInt(value.replace('saved-', ''), 10);
      const savedPreset = settings.savedPresets[idx];
      if (savedPreset) {
        update({
          bufferPreset: value,
          customJitterConfig: savedPreset.config,
        });
      }
    } else if (value === 'custom') {
      // Selecting generic "Custom" = start fresh from Auto preset config
      const autoPreset = JITTER_PRESETS.find((p) => p.id === 'auto');
      const autoConfig = autoPreset?.config ?? settings.customJitterConfig;
      update({
        bufferPreset: 'custom',
        customJitterConfig: autoConfig,
      });
    } else {
      update({ bufferPreset: value as PresetId });
    }
  };

  // Determine the selected value for the UI dropdown
  let selectedValue = settings.bufferPreset as string;

  return (
    <div>
      <CustomSelect
        id="setting-preset"
        options={options}
        value={selectedValue}
        onChange={handleChange}
      />
    </div>
  );
}
