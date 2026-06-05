import { useMemo, useCallback, useRef, useState } from 'react';
import { useSettings } from '../../hooks/use-settings';
import { CustomSelect, type SelectOption } from '../shared/CustomSelect';
import { JITTER_PRESETS } from '../../core/presets';
import type { PresetId } from '../../core/types';
import { NoBufferWarningDialog } from './NoBufferWarning';

const LS_KEY = 'gemacast_nobuffer_warning_dismissed';

function isWarningDismissed(): boolean {
  try {
    return localStorage.getItem(LS_KEY) === 'true';
  } catch {
    return false;
  }
}

function dismissWarning() {
  try {
    localStorage.setItem(LS_KEY, 'true');
  } catch {
    // Ignore storage errors
  }
}

export function BufferPresetSelect() {
  const { settings, update } = useSettings();
  const warningDialogRef = useRef<HTMLDialogElement>(null);
  const [dontShowAgain, setDontShowAgain] = useState(false);

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

  const applyNoBuffer = useCallback(() => {
    update({ bufferPreset: 'nobuffer' as PresetId });
  }, [update]);

  const handleWarningOk = useCallback(() => {
    if (dontShowAgain) {
      dismissWarning();
    }
    warningDialogRef.current?.close();
    applyNoBuffer();
  }, [dontShowAgain, applyNoBuffer]);

  const handleChange = (value: string) => {
    if (value === 'nobuffer') {
      if (isWarningDismissed()) {
        applyNoBuffer();
      } else {
        warningDialogRef.current?.showModal();
      }
      return;
    }

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
  const selectedValue = settings.bufferPreset as string;

  return (
    <div>
      <CustomSelect
        id="setting-preset"
        options={options}
        value={selectedValue}
        onChange={handleChange}
        renderOption={(option) => (
          <>
            <span className={`font-medium ${option.value === 'nobuffer' ? 'text-red-500' : ''}`}>
              {option.label}
            </span>
            {option.description && (
              <span className="mt-0.5 text-xs text-muted-foreground">{option.description}</span>
            )}
          </>
        )}
      />

      <NoBufferWarningDialog
        dialogRef={warningDialogRef}
        dontShowAgain={dontShowAgain}
        setDontShowAgain={setDontShowAgain}
        handleOk={handleWarningOk}
      />
    </div>
  );
}
