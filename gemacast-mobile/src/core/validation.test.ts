import { describe, it, expect } from 'bun:test';
import { validateJitterConfig, isJitterConfigEqual, getDefaultResetConfig } from './validation';
import type { JitterConfig, AppSettings } from './types';

const validStaticConfig: JitterConfig = {
  minDepthMs: 25,
  comfortCapMs: 1000,
  peakDecayHalflifeMs: 0,
  resumeThresholdPct: 0.25,
  staticTargetMs: 60,
};

describe('validateJitterConfig', () => {
  it('accepts a valid static config', () => {
    const result = validateJitterConfig(validStaticConfig);
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  it('accepts a valid static config with large buffer depth', () => {
    const result = validateJitterConfig({ ...validStaticConfig, staticTargetMs: 500 });
    expect(result.valid).toBe(true);
  });

  it('accepts staticTargetMs = 0 (no buffer)', () => {
    const config = { ...validStaticConfig, staticTargetMs: 0 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(true);
  });

  it('rejects negative staticTargetMs', () => {
    const config = { ...validStaticConfig, staticTargetMs: -10 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors[0].field).toBe('staticTargetMs');
  });

  it('rejects NaN staticTargetMs', () => {
    const config = { ...validStaticConfig, staticTargetMs: NaN };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors[0].field).toBe('staticTargetMs');
  });

  it('rejects null staticTargetMs (adaptive mode no longer allowed)', () => {
    const config = { ...validStaticConfig, staticTargetMs: null };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors[0].field).toBe('staticTargetMs');
  });

  it('rejects undefined staticTargetMs', () => {
    const config = { ...validStaticConfig, staticTargetMs: undefined };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors[0].field).toBe('staticTargetMs');
  });
});

describe('isJitterConfigEqual', () => {
  it('returns true for identical configs', () => {
    expect(isJitterConfigEqual(validStaticConfig, { ...validStaticConfig })).toBe(true);
  });

  it('returns false for different staticTargetMs', () => {
    expect(
      isJitterConfigEqual(validStaticConfig, { ...validStaticConfig, staticTargetMs: 999 }),
    ).toBe(false);
  });

  it('treats null and undefined staticTargetMs as equal', () => {
    const a = { ...validStaticConfig, staticTargetMs: undefined };
    const b = { ...validStaticConfig, staticTargetMs: null };
    expect(isJitterConfigEqual(a, b)).toBe(true);
  });

  it('returns false when one has staticTargetMs and other does not', () => {
    const a = { ...validStaticConfig };
    const b = { ...validStaticConfig, staticTargetMs: undefined };
    expect(isJitterConfigEqual(a, b)).toBe(false);
  });
});

describe('getDefaultResetConfig', () => {
  it('returns Auto config when no saved match', () => {
    const settings = {
      customJitterConfig: validStaticConfig,
      savedPresets: [],
      bufferPreset: 'custom',
    } as unknown as AppSettings;
    const config = getDefaultResetConfig(settings);
    expect(config.minDepthMs).toBe(25); // Auto preset
  });
});
