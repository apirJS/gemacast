import type { AppSettings, JitterConfig } from './types';
import { JITTER_PRESETS } from './presets';

export type FieldError = {
  field: string;
  message: string;
};

export type ValidationResult = {
  valid: boolean;
  errors: FieldError[];
};

/**
 * Checks whether a numeric value is a valid, non-empty finite number.
 * Rejects NaN, Infinity, empty string coercion (which produces NaN), and undefined.
 */
function isValidNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/**
 * Validate a JitterConfig for use as a custom preset.
 * Returns field-level errors for any invalid values.
 */
export function validateJitterConfig(config: JitterConfig): ValidationResult {
  const errors: FieldError[] = [];

  // Custom presets are always static — validate the buffer depth.
  if (config.staticTargetMs != null) {
    if (!isValidNumber(config.staticTargetMs) || config.staticTargetMs < 0) {
      errors.push({ field: 'staticTargetMs', message: 'Must be ≥ 0' });
    }
  } else {
    // staticTargetMs is required for custom presets
    errors.push({ field: 'staticTargetMs', message: 'Buffer depth is required' });
  }

  return { valid: errors.length === 0, errors };
}

/**
 * Deep equality check for two JitterConfig objects.
 */
export function isJitterConfigEqual(a: JitterConfig, b: JitterConfig): boolean {
  return (
    a.minDepthMs === b.minDepthMs &&
    a.comfortCapMs === b.comfortCapMs &&
    a.peakDecayHalflifeMs === b.peakDecayHalflifeMs &&
    a.resumeThresholdPct === b.resumeThresholdPct &&
    (a.staticTargetMs ?? null) === (b.staticTargetMs ?? null)
  );
}

/**
 * Get the default config to use when the user clicks "Reset".
 * - If the user is editing a saved preset, returns that saved preset's config.
 * - Otherwise, returns the Auto preset's config.
 */
export function getDefaultResetConfig(settings: AppSettings): JitterConfig {
  const currentConfig = settings.customJitterConfig;

  // Check if current config matches a saved preset
  const savedMatch = settings.savedPresets.find((sp) =>
    isJitterConfigEqual(sp.config, currentConfig),
  );

  if (savedMatch) {
    return { ...savedMatch.config };
  }

  // Fall back to Auto preset
  const autoPreset = JITTER_PRESETS.find((p) => p.id === 'auto');
  return autoPreset?.config ? { ...autoPreset.config } : { ...currentConfig };
}
