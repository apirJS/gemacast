import { useState, useMemo, useCallback, useEffect } from 'react';
import { useSettings } from './use-settings';
import type { JitterConfig } from '../core/types';
import { validateJitterConfig, isJitterConfigEqual } from '../core/validation';

export type CustomPresetEditorState = {
  /** Whether the editor is visible (bufferPreset === 'custom') */
  isCustom: boolean;
  /** Whether the current config matches a saved preset */
  isEditingSaved: boolean;
  /** Index of matching saved preset, or -1 */
  savedMatchIndex: number;
  /** The current jitter config being edited */
  config: JitterConfig;
  /** Preset name input value */
  presetName: string;
  /** Whether the config is valid */
  isValid: boolean;
  /** Validation errors for specific fields */
  validationErrors: Array<{ field: string; message: string }>;
  /** Whether the save button should be enabled */
  canSave: boolean;
  /** Whether delete confirmation dialog is open */
  isDeleteDialogOpen: boolean;
};

export type CustomPresetEditorActions = {
  setPresetName: (name: string) => void;
  updateField: (patch: Partial<JitterConfig>) => void;
  handleSave: () => void;
  handleReset: () => void;
  requestDelete: () => void;
  confirmDelete: () => void;
  cancelDelete: () => void;
};

/**
 * Default config for new custom presets: static 60ms buffer.
 */
function getDefaultCustomConfig(): JitterConfig {
  return {
    minDepthMs: 25,
    comfortCapMs: 1000,
    peakDecayHalflifeMs: 0,
    resumeThresholdPct: 0.25,
    staticTargetMs: 0,
  };
}

export function useCustomPresetEditor(): CustomPresetEditorState & CustomPresetEditorActions {
  const { settings, update } = useSettings();
  const [config, setConfig] = useState(settings.customJitterConfig);
  const isCustom = settings.bufferPreset === 'custom' || settings.bufferPreset.startsWith('saved-');

  const [presetName, setPresetName] = useState('');
  const [isDeleteDialogOpen, setIsDeleteDialogOpen] = useState(false);

  const savedMatchIndex = settings.bufferPreset.startsWith('saved-')
    ? parseInt(settings.bufferPreset.replace('saved-', ''), 10)
    : -1;

  const isEditingSaved = savedMatchIndex >= 0;

  // When editing a saved preset, ensure the name field is initialized to the saved preset's name.
  // When creating a new custom preset (not saved), ensure the name field starts empty.
  useEffect(() => {
    if (isEditingSaved) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setPresetName(settings.savedPresets[savedMatchIndex]?.name ?? '');
    } else {
      setPresetName('');
    }
    const selectedConfig = settings.bufferPreset.startsWith('saved-')
      ? settings.savedPresets[savedMatchIndex]?.config
      : settings.customJitterConfig;
    setConfig(
      selectedConfig?.staticTargetMs == null
        ? { ...(selectedConfig ?? getDefaultCustomConfig()), staticTargetMs: 0 }
        : selectedConfig,
    );
  }, [
    settings.bufferPreset,
    isEditingSaved,
    savedMatchIndex,
    settings.savedPresets,
    settings.customJitterConfig,
  ]);

  const validation = useMemo(() => validateJitterConfig(config), [config]);
  const isValid = validation.valid;

  const canSave = useMemo(() => {
    if (!presetName.trim()) return false;
    if (!isValid) return false;

    if (isEditingSaved) {
      const original = settings.savedPresets[savedMatchIndex];
      if (
        original &&
        original.name === presetName.trim() &&
        isJitterConfigEqual(original.config, config)
      ) {
        return false; // No changes to save
      }
    } else {
      const existingByName = settings.savedPresets.find((sp) => sp.name === presetName.trim());
      if (existingByName && isJitterConfigEqual(existingByName.config, config)) {
        return false; // No changes to save
      }
    }

    return true;
  }, [presetName, isValid, settings.savedPresets, config, isEditingSaved, savedMatchIndex]);

  const updateField = useCallback(
    (patch: Partial<JitterConfig>) => {
      setConfig((current) => ({ ...current, ...patch }));
    },
    [],
  );

  const handleSave = useCallback(() => {
    if (!canSave) return;

    const trimmedName = presetName.trim();
    const saved = [...settings.savedPresets];
    let newBufferPreset = settings.bufferPreset as string;

    if (isEditingSaved && saved[savedMatchIndex]) {
      // UPDATE existing preset
      saved[savedMatchIndex] = { name: trimmedName, config };
    } else {
      // CREATE new preset (or overwrite if name exactly matches another one)
      const existingIndex = saved.findIndex((sp) => sp.name === trimmedName);
      if (existingIndex >= 0) {
        saved[existingIndex] = { name: trimmedName, config };
        newBufferPreset = `saved-${existingIndex}`;
      } else {
        saved.push({ name: trimmedName, config });
        newBufferPreset = `saved-${saved.length - 1}`;
      }
    }

    void update({
      savedPresets: saved,
      bufferPreset: newBufferPreset,
      customJitterConfig: config,
    });
  }, [
    canSave,
    presetName,
    config,
    settings.savedPresets,
    update,
    isEditingSaved,
    savedMatchIndex,
    settings.bufferPreset,
  ]);

  const handleReset = useCallback(() => {
    if (isEditingSaved) {
      // Reset to the saved preset's original config
      const savedConfig = settings.savedPresets[savedMatchIndex].config;
      // Ensure the saved config has staticTargetMs set (migrate legacy adaptive presets)
      const migratedConfig =
        savedConfig.staticTargetMs == null ? { ...savedConfig, staticTargetMs: 0 } : savedConfig;
      setConfig(migratedConfig);
      void update({ customJitterConfig: migratedConfig });
      setPresetName(settings.savedPresets[savedMatchIndex].name);
    } else {
      // Reset to default static config for new custom presets
      const defaultConfig = getDefaultCustomConfig();
      setConfig(defaultConfig);
      void update({ customJitterConfig: defaultConfig });
      setPresetName('');
    }
  }, [isEditingSaved, savedMatchIndex, settings.savedPresets, update]);

  /**
   * Delete preset. After deletion, reset to default static config for creating new.
   */
  const requestDelete = useCallback(() => {
    setIsDeleteDialogOpen(true);
  }, []);

  const confirmDelete = useCallback(() => {
    if (savedMatchIndex >= 0) {
      const saved = [...settings.savedPresets];
      saved.splice(savedMatchIndex, 1);

      // After deleting, load default static config for creating new
      const defaultConfig = getDefaultCustomConfig();
      void update({
        savedPresets: saved,
        bufferPreset: 'custom',
        customJitterConfig: defaultConfig,
      });
      setPresetName('');
    }
    setIsDeleteDialogOpen(false);
  }, [savedMatchIndex, settings.savedPresets, update]);

  const cancelDelete = useCallback(() => {
    setIsDeleteDialogOpen(false);
  }, []);

  return {
    isCustom,
    isEditingSaved,
    savedMatchIndex,
    config,
    presetName,
    isValid,
    validationErrors: validation.errors,
    canSave,
    isDeleteDialogOpen,
    setPresetName,
    updateField,
    handleSave,
    handleReset,
    requestDelete,
    confirmDelete,
    cancelDelete,
  };
}
