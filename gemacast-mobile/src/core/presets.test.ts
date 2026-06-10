import { describe, it, expect } from 'bun:test';
import { getPresetConfig, JITTER_PRESETS } from './presets';

describe('getPresetConfig', () => {
  const fallback = {
    minDepthMs: 999,
    comfortCapMs: 999,
    peakDecayHalflifeMs: 999,
    resumeThresholdPct: 0.99,
  };

  it('returns auto preset config', () => {
    const config = getPresetConfig('auto', fallback);
    expect(config.minDepthMs).toBe(5);
    expect(config.comfortCapMs).toBe(1000);
  });

  it('returns wired preset config', () => {
    const config = getPresetConfig('wired', fallback);
    expect(config.minDepthMs).toBe(2);
  });

  it('returns custom config for "custom" preset', () => {
    const config = getPresetConfig('custom', fallback);
    expect(config).toBe(fallback);
  });

  it('falls back to customConfig for unknown ID', () => {
    const config = getPresetConfig('nonexistent', fallback);
    expect(config).toBe(fallback);
  });
});

describe('JITTER_PRESETS data integrity', () => {
  it('all presets have unique IDs', () => {
    const ids = JITTER_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('all presets except custom have non-null config', () => {
    for (const preset of JITTER_PRESETS) {
      if (preset.id === 'custom') {
        expect(preset.config).toBeNull();
      } else {
        expect(preset.config).not.toBeNull();
      }
    }
  });
});
