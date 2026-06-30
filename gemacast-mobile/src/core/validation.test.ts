import { describe, it, expect } from 'bun:test';
import { validateJitterConfig, isJitterConfigEqual, getDefaultResetConfig } from './validation';
import type { JitterConfig, AppSettings } from './types';
import { ConnectionMode } from './types';

const validAdaptiveConfig: JitterConfig = {
  minDepthMs: 25,
  comfortCapMs: 150,
  peakDecayHalflifeMs: 3500,
  resumeThresholdPct: 0.4,
};

const validStaticConfig: JitterConfig = {
  minDepthMs: 25,
  comfortCapMs: 150,
  peakDecayHalflifeMs: 3500,
  resumeThresholdPct: 0.4,
  staticTargetMs: 60,
};

describe('validateJitterConfig', () => {
  it('accepts a valid adaptive config', () => {
    const result = validateJitterConfig(validAdaptiveConfig);
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  it('accepts a valid static config', () => {
    const result = validateJitterConfig(validStaticConfig);
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  it('accepts zero values for adaptive fields', () => {
    const config: JitterConfig = {
      minDepthMs: 0,
      comfortCapMs: 0,
      peakDecayHalflifeMs: 0,
      resumeThresholdPct: 0,
    };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(true);
  });

  it('rejects NaN minDepthMs', () => {
    const config = { ...validAdaptiveConfig, minDepthMs: NaN };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'minDepthMs')).toBe(true);
  });

  it('rejects negative minDepthMs', () => {
    const config = { ...validAdaptiveConfig, minDepthMs: -1 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'minDepthMs')).toBe(true);
  });

  it('rejects comfortCapMs < minDepthMs', () => {
    const config = { ...validAdaptiveConfig, minDepthMs: 100, comfortCapMs: 50 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'comfortCapMs')).toBe(true);
  });

  it('rejects resumeThresholdPct > 1', () => {
    const config = { ...validAdaptiveConfig, resumeThresholdPct: 1.5 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'resumeThresholdPct')).toBe(true);
  });

  it('rejects resumeThresholdPct < 0', () => {
    const config = { ...validAdaptiveConfig, resumeThresholdPct: -0.1 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'resumeThresholdPct')).toBe(true);
  });

  it('rejects staticTargetMs <= 0 in static mode', () => {
    const config = { ...validStaticConfig, staticTargetMs: 0 };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'staticTargetMs')).toBe(true);
  });

  it('rejects NaN staticTargetMs in static mode', () => {
    const config = { ...validStaticConfig, staticTargetMs: NaN };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.field === 'staticTargetMs')).toBe(true);
  });

  it('only validates static field in static mode', () => {
    // Even with bad adaptive values, static mode only checks staticTargetMs
    const config: JitterConfig = {
      minDepthMs: -999,
      comfortCapMs: -999,
      peakDecayHalflifeMs: -999,
      resumeThresholdPct: -999,
      staticTargetMs: 60,
    };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(true);
  });

  it('reports multiple errors at once', () => {
    const config: JitterConfig = {
      minDepthMs: NaN,
      comfortCapMs: NaN,
      peakDecayHalflifeMs: -1,
      resumeThresholdPct: 5,
    };
    const result = validateJitterConfig(config);
    expect(result.valid).toBe(false);
    expect(result.errors.length).toBeGreaterThanOrEqual(3);
  });
});

describe('isJitterConfigEqual', () => {
  it('returns true for identical configs', () => {
    expect(isJitterConfigEqual(validAdaptiveConfig, { ...validAdaptiveConfig })).toBe(true);
  });

  it('returns false for different minDepthMs', () => {
    expect(
      isJitterConfigEqual(validAdaptiveConfig, { ...validAdaptiveConfig, minDepthMs: 999 }),
    ).toBe(false);
  });

  it('treats undefined and null staticTargetMs as equal', () => {
    const a = { ...validAdaptiveConfig, staticTargetMs: undefined };
    const b = { ...validAdaptiveConfig, staticTargetMs: null };
    expect(isJitterConfigEqual(a, b)).toBe(true);
  });

  it('returns false when one has static and other does not', () => {
    const a = { ...validAdaptiveConfig };
    const b = { ...validAdaptiveConfig, staticTargetMs: 60 };
    expect(isJitterConfigEqual(a, b)).toBe(false);
  });
});

describe('getDefaultResetConfig', () => {
  const makeSettings = (overrides: Partial<AppSettings> = {}): AppSettings => ({
    theme: 'dark',
    mode: ConnectionMode.Wifi,
    exclusiveMode: false,
    keepScreenOn: false,
    bufferPreset: 'custom',
    customJitterConfig: validAdaptiveConfig,
    savedPresets: [],
    bitratePreset: '128',
    customBitrateKbps: 128,
    gainDb: 0,
    ...overrides,
  });

  it('returns Auto preset config when no saved preset matches', () => {
    const settings = makeSettings();
    const result = getDefaultResetConfig(settings);
    // Auto preset values from presets.ts
    expect(result.minDepthMs).toBe(8);
    expect(result.comfortCapMs).toBe(1500);
  });

  it('returns saved preset config when current config matches a saved preset', () => {
    const savedConfig: JitterConfig = {
      minDepthMs: 42,
      comfortCapMs: 200,
      peakDecayHalflifeMs: 1000,
      resumeThresholdPct: 0.5,
    };
    const settings = makeSettings({
      customJitterConfig: savedConfig,
      savedPresets: [{ name: 'My Preset', config: savedConfig }],
    });
    const result = getDefaultResetConfig(settings);
    expect(result.minDepthMs).toBe(42);
    expect(result.comfortCapMs).toBe(200);
  });
});
