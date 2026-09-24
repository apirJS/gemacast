import { useRef, useState } from 'react';
import { settingsController, useSettingsController } from '../../controllers';
import { CustomSelect, type SelectOption } from '../shared/CustomSelect';
import { JITTER_PRESETS } from '../../core/presets';
import { NoBufferWarningDialog } from './NoBufferWarning';

export function BufferPresetSelect() {
  const { settings } = useSettingsController();
  const warningDialogRef = useRef<HTMLDialogElement>(null);
  const [dontShowAgain, setDontShowAgain] = useState(false);

  const options: SelectOption<string>[] = (() => {
    const builtIn = JITTER_PRESETS.map((p) => ({
      value: p.id,
      label: p.name,
      description: p.description,
    }));

    const saved = settings.savedPresets.map((sp, i) => ({
      value: `saved-${i}`,
      label: sp.name,
      description:
        sp.config.staticTargetMs != null
          ? `Fixed ${sp.config.staticTargetMs}ms buffer`
          : 'User-saved preset',
    }));

    return [...builtIn, ...saved];
  })();

  const applyNoBuffer = () => {
    void settingsController.selectBufferPreset('nobuffer');
  };

  const handleWarningOk = () => {
    if (dontShowAgain) {
      settingsController.dismissNoBufferWarning();
    }
    warningDialogRef.current?.close();
    applyNoBuffer();
  };

  const handleChange = (value: string) => {
    if (value === 'nobuffer') {
      if (!settingsController.requiresNoBufferWarning()) {
        applyNoBuffer();
      } else {
        warningDialogRef.current?.showModal();
      }
      return;
    }

    void settingsController.selectBufferPreset(value);
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
            <span
              className={`font-medium ${option.value === 'nobuffer' ? 'text-destructive' : ''}`}
            >
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
