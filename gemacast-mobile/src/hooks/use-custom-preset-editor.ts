import { useState, useMemo, useCallback, useEffect } from 'react';
import { useSettings } from './use-settings';
import type { JitterConfig } from '../core/types';
import {
  validateJitterConfig,
  isJitterConfigEqual,
} from '../core/validation';
import { JITTER_PRESETS } from '../core/presets';

export type BufferMode = 'static' | 'adaptive';

export type CustomPresetEditorState = {
  /** Whether the editor is visible (bufferPreset === 'custom') */
  isCustom: boolean;
  /** Whether the current config matches a saved preset */
  isEditingSaved: boolean;
  /** Index of matching saved preset, or -1 */
  savedMatchIndex: number;
  /** The current jitter config being edited */
  config: JitterConfig;
  /** Current buffer mode derived from config */
  bufferMode: BufferMode;
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
  setBufferMode: (mode: BufferMode) => void;
  updateField: (patch: Partial<JitterConfig>) => void;
  handleSave: () => void;
  handleReset: () => void;
  requestDelete: () => void;
  confirmDelete: () => void;
  cancelDelete: () => void;
};

/**
 * Get the "Auto" preset config as the base for creating new custom presets.
 */
function getAutoConfig(): JitterConfig {
  const auto = JITTER_PRESETS.find((p) => p.id === 'auto');
  return auto?.config ?? {
    minDepthMs: 5,
    comfortCapMs: 1000,
    peakDecayHalflifeMs: 0,
    resumeThresholdPct: 0.25,
  };
}

export function useCustomPresetEditor(): CustomPresetEditorState & CustomPresetEditorActions {
  const { settings, update } = useSettings();
  const config = settings.customJitterConfig;
  const isCustom =
    settings.bufferPreset === 'custom' || settings.bufferPreset.startsWith('saved-');

  const [presetName, setPresetName] = useState('');
  const [bufferMode, setBufferModeState] = useState<BufferMode>(
    config.staticTargetMs != null ? 'static' : 'adaptive',
  );
  const [isDeleteDialogOpen, setIsDeleteDialogOpen] = useState(false);

  const savedMatchIndex = settings.bufferPreset.startsWith('saved-')
    ? parseInt(settings.bufferPreset.replace('saved-', ''), 10)
    : -1;

  const isEditingSaved = savedMatchIndex >= 0;

  // When editing a saved preset, ensure the name field is initialized to the saved preset's name.
  // When creating a new custom preset (not saved), ensure the name field starts empty.
  useEffect(() => {
    if (isEditingSaved) {
      setPresetName(settings.savedPresets[savedMatchIndex]?.name ?? '');
    } else {
      setPresetName('');
    }
  }, [settings.bufferPreset, isEditingSaved, savedMatchIndex, settings.savedPresets]);

  // Sync buffer mode when config changes externally (e.g. selecting a saved preset from dropdown)
  useEffect(() => {
    setBufferModeState(config.staticTargetMs != null && !Number.isNaN(config.staticTargetMs) ? 'static' : 'adaptive');
  }, [settings.bufferPreset]);

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
      const existingByName = settings.savedPresets.find(
        (sp) => sp.name === presetName.trim(),
      );
      if (existingByName && isJitterConfigEqual(existingByName.config, config)) {
        return false; // No changes to save
      }
    }

    return true;
  }, [presetName, isValid, settings.savedPresets, config, isEditingSaved, savedMatchIndex]);

  const updateField = useCallback(
    (patch: Partial<JitterConfig>) => {
      update({ customJitterConfig: { ...config, ...patch } });
    },
    [config, update],
  );

  const setBufferMode = useCallback(
    (mode: BufferMode) => {
      setBufferModeState(mode);
      if (mode === 'static') {
        updateField({ staticTargetMs: 60 });
      } else {
        updateField({ staticTargetMs: null });
      }
    },
    [updateField],
  );

  const handleSave = useCallback(() => {
    if (!canSave) return;

    const trimmedName = presetName.trim();
    let saved = [...settings.savedPresets];
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

    update({
      savedPresets: saved,
      bufferPreset: newBufferPreset,
      customJitterConfig: config,
    });
  }, [canSave, presetName, config, settings.savedPresets, update, isEditingSaved, savedMatchIndex, settings.bufferPreset]);

  /**
   * Reset fields.
   * - If editing a saved preset → reset to saved preset's config
   * - If creating new → reset to Auto preset values and clear the name
   */
  const handleReset = useCallback(() => {
    if (isEditingSaved) {
      // Reset to the saved preset's original config
      const savedConfig = settings.savedPresets[savedMatchIndex].config;
      update({ customJitterConfig: savedConfig });
      setPresetName(settings.savedPresets[savedMatchIndex].name);
      setBufferModeState(savedConfig.staticTargetMs != null && !Number.isNaN(savedConfig.staticTargetMs) ? 'static' : 'adaptive');
    } else {
      // Reset to Auto preset for new custom presets
      const autoConfig = getAutoConfig();
      update({ customJitterConfig: autoConfig });
      setPresetName('');
      setBufferModeState(autoConfig.staticTargetMs != null && !Number.isNaN(autoConfig.staticTargetMs) ? 'static' : 'adaptive');
    }
  }, [isEditingSaved, savedMatchIndex, settings.savedPresets, update]);

  /**
   * Delete preset. After deletion, reset to Auto preset values for creating new.
   */
  const requestDelete = useCallback(() => {
    setIsDeleteDialogOpen(true);
  }, []);

  const confirmDelete = useCallback(() => {
    if (savedMatchIndex >= 0) {
      const saved = [...settings.savedPresets];
      saved.splice(savedMatchIndex, 1);

      // After deleting, load Auto config for creating new
      const autoConfig = getAutoConfig();
      update({
        savedPresets: saved,
        bufferPreset: 'custom',
        customJitterConfig: autoConfig,
      });
      setPresetName('');
      setBufferModeState(autoConfig.staticTargetMs != null && !Number.isNaN(autoConfig.staticTargetMs) ? 'static' : 'adaptive');
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
    bufferMode,
    presetName,
    isValid,
    validationErrors: validation.errors,
    canSave,
    isDeleteDialogOpen,
    setPresetName,
    setBufferMode,
    updateField,
    handleSave,
    handleReset,
    requestDelete,
    confirmDelete,
    cancelDelete,
  };
}
