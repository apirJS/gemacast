import React, { useState } from 'react';
import { useSettings } from '../../hooks/use-settings';
import { CustomSelect, type SelectOption } from '../shared/CustomSelect';
import type { BitratePreset } from '../../core/types';

const BITRATE_OPTIONS: SelectOption<BitratePreset>[] = [
  { value: '10', label: '10 Kbps', description: 'Voice quality' },
  { value: '24', label: '24 Kbps', description: 'Low quality' },
  { value: '32', label: '32 Kbps', description: 'FM Radio quality' },
  { value: '64', label: '64 Kbps', description: 'Standard quality' },
  { value: '96', label: '96 Kbps', description: 'Good quality' },
  { value: '128', label: '128 Kbps — High (Default)', description: 'Recommended for most use' },
  { value: '256', label: '256 Kbps', description: 'Very high quality' },
  { value: '450', label: '450 Kbps', description: 'Near-transparent' },
  { value: '512', label: '512 Kbps', description: 'Maximum Opus quality' },
  { value: 'raw', label: 'Uncompressed PCM', description: 'Zero latency codec path — very high bandwidth' },
  { value: 'custom', label: 'Custom Bitrate', description: 'Specify your own bitrate value' },
];

export function BitrateSelect() {
  const { settings, update } = useSettings();
  const [customKbps, setCustomKbps] = useState(String(settings.customBitrateKbps));

  const handleSelect = (value: BitratePreset) => {
    update({ bitratePreset: value });
  };

  const applyCustom = () => {
    const val = parseInt(customKbps, 10);
    if (val >= 6 && val <= 512) {
      update({ customBitrateKbps: val, bitratePreset: 'custom' });
    }
  };

  const options = React.useMemo(() => {
    return BITRATE_OPTIONS.map((opt) => {
      if (opt.value === 'custom' && settings.bitratePreset === 'custom') {
        return {
          ...opt,
          label: `Custom - ${settings.customBitrateKbps} Kbps`,
        };
      }
      return opt;
    });
  }, [settings.bitratePreset, settings.customBitrateKbps]);

  return (
    <div>
      <CustomSelect
        id="setting-bitrate"
        options={options}
        value={settings.bitratePreset}
        onChange={handleSelect}
      />

      {settings.bitratePreset === 'custom' && (
        <div className="mt-2 flex items-center gap-2 animate-[fade-in_200ms_ease-out]">
          <input
            type="number"
            value={customKbps}
            onChange={(e) => setCustomKbps(e.target.value)}
            placeholder="128"
            min={6}
            max={512}
            className="flex-1 mr-1 rounded-[4px] border border-border bg-background px-2 py-1 text-left text-base text-foreground outline-none focus:border-primary focus:ring-1 focus:ring-primary"
          />
          <span className="text-[0.9rem] font-medium text-muted-foreground">Kbps</span>
          <button
            type="button"
            className="rounded-[6px] bg-primary px-[0.9rem] py-[0.45rem] text-[0.85rem] font-semibold text-primary-foreground transition-opacity hover:opacity-90 active:opacity-80 disabled:cursor-not-allowed disabled:opacity-40"
            onClick={applyCustom}
            disabled={!customKbps || parseInt(customKbps, 10) < 6 || parseInt(customKbps, 10) > 512}
          >
            Apply
          </button>
        </div>
      )}
    </div>
  );
}
